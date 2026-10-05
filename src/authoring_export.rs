//! Read-only, content-hashed authoring projection for Content Tools.
//!
//! The semantic tree determines identities and values. Parser annotations and
//! original bytes determine editable fragments; expanded or ambiguous positions
//! never acquire write authority merely because they resolve to a symbol.

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use dreammaker::annotation::{Annotation, AnnotationTree};
use dreammaker::ast::Ident;
use dreammaker::constants::{Arguments, Constant};
use dreammaker::lexer::{LocatedToken, Punctuation, Token};
use dreammaker::objtree::{ObjectTree, TypeRef};
use dreammaker::preprocessor::{Define, DefineHistory};
use dreammaker::{Context, Location, Parser, Preprocessor, ReadPolicy, Severity};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::atomic_output::{write_atomic, AtomicOutputError};
use crate::index::ReferenceKind;
use crate::spaceman::language::ReferenceTable;
use crate::PathPolicy;

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct SourceSpan {
    pub path: String,
    pub start: usize,
    pub end: usize,
    /// SHA-256 of the entire physical input file, including its BOM and CRLF.
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct InputFile {
    pub path: String,
    pub sha256: String,
    pub encoding: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldOccurrence {
    pub source: Option<SourceSpan>,
    pub expression: Option<String>,
    pub editable: bool,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub name: String,
    pub expression: Option<String>,
    pub value: Value,
    pub value_known: bool,
    pub owner_type: String,
    pub source: Option<SourceSpan>,
    pub editable: bool,
    pub local: bool,
    pub occurrences: Vec<FieldOccurrence>,
}

#[derive(Debug, Serialize)]
pub struct Procedure {
    pub name: String,
    pub owner_type: String,
    pub source: Option<SourceSpan>,
    pub text: String,
    pub editable: bool,
    pub override_index: usize,
    pub local: bool,
}

#[derive(Debug, Serialize)]
pub struct Reference {
    pub id: String,
    pub target_type: String,
    pub source: SourceSpan,
    pub editable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Definition {
    pub type_path: String,
    pub parent_type: Option<String>,
    pub kind: &'static str,
    pub source: Option<SourceSpan>,
    pub occurrences: Vec<SourceSpan>,
    pub fields: Vec<Field>,
    pub procedures: Vec<Procedure>,
    pub references: Vec<Reference>,
}

#[derive(Debug, Serialize)]
pub struct MacroConstant {
    pub name: String,
    pub expression: String,
    pub value: Value,
    pub value_known: bool,
    pub source: Option<SourceSpan>,
}

#[derive(Debug, Serialize)]
pub struct AnalysisConfiguration {
    pub profile: &'static str,
    pub command_line_defines: Vec<String>,
    pub config_file: Option<String>,
    pub builtin_defines: BTreeMap<String, Value>,
    pub when_compile_defined_at_end: bool,
}

#[derive(Debug, Serialize)]
pub struct AuthoringExport {
    pub schema_version: u32,
    pub analyzer_version: String,
    pub spacemandmm_revision: &'static str,
    pub spacemandmm_local_patch_sha256: &'static str,
    pub environment: String,
    pub configuration: AnalysisConfiguration,
    pub input_files: Vec<InputFile>,
    pub constants: Vec<MacroConstant>,
    pub definitions: Vec<Definition>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
struct PhysicalFile {
    path: String,
    bytes: Vec<u8>,
    sha256: String,
    line_starts: Vec<usize>,
    utf8: bool,
}

impl PhysicalFile {
    fn new(path: String, bytes: Vec<u8>) -> Self {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let mut line_starts = vec![if bytes.starts_with(b"\xef\xbb\xbf") {
            3
        } else {
            0
        }];
        line_starts.extend(
            bytes
                .iter()
                .enumerate()
                .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
        );
        let utf8 = std::str::from_utf8(&bytes).is_ok();
        Self {
            path,
            bytes,
            sha256,
            line_starts,
            utf8,
        }
    }

    fn offset(&self, location: Location) -> Option<usize> {
        let line = usize::try_from(location.line.checked_sub(1)?).ok()?;
        let start = *self.line_starts.get(line)?;
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        if location.column == u16::MAX {
            return Some(end);
        }
        let offset = start.checked_add(usize::from(location.column.checked_sub(1)?))?;
        (offset <= end).then_some(offset)
    }

    fn span(&self, start: usize, end: usize) -> Option<SourceSpan> {
        (start < end && end <= self.bytes.len()).then(|| SourceSpan {
            path: self.path.clone(),
            start,
            end,
            sha256: self.sha256.clone(),
        })
    }

    fn text(&self, span: &SourceSpan) -> String {
        dreammaker::lexer::from_utf8_or_latin1_borrowed(&self.bytes[span.start..span.end])
            .into_owned()
    }

    fn trim_span(&self, mut start: usize, mut end: usize) -> Option<SourceSpan> {
        while start < end && self.bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        while start < end && self.bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        self.span(start, end)
    }

    fn token_span(&self, location: Location) -> Option<SourceSpan> {
        let start = self.offset(location)?;
        let mut end = start;
        while self
            .bytes
            .get(end)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'/' | b'.' | b':'))
        {
            end += 1;
        }
        self.span(start, end)
    }

    fn initializer_span(&self, whole: &SourceSpan) -> Option<SourceSpan> {
        let scratch = Context::default();
        let bytes = &self.bytes[whole.start..whole.end];
        let relative = PhysicalFile::new(String::new(), bytes.to_vec());
        let tokens = dreammaker::Lexer::new(&scratch, Location::BUILTINS.file, bytes)
            .filter(|token| {
                !matches!(
                    token.token,
                    Token::Punct(Punctuation::Newline | Punctuation::Tab | Punctuation::Space)
                        | Token::DocComment(_)
                )
            })
            .collect::<Vec<_>>();
        let mut depth = 0_usize;
        let mut assignment = None;
        for (position, token) in tokens.iter().enumerate() {
            match token.token {
                Token::Punct(Punctuation::LParen | Punctuation::LBracket | Punctuation::LBrace) => {
                    depth += 1
                }
                Token::Punct(Punctuation::RParen | Punctuation::RBracket | Punctuation::RBrace) => {
                    depth = depth.saturating_sub(1)
                }
                Token::Punct(Punctuation::Assign) if depth == 0 => {
                    assignment = Some(position);
                    break;
                }
                _ => {}
            }
        }
        let first = assignment? + 1;
        let start = relative.offset(tokens.get(first)?.location)?;
        let consumed = Rc::new(Cell::new(0_usize));
        let counter = consumed.clone();
        let eof = LocatedToken::new(tokens[first].location, Token::Eof);
        let stream = tokens[first..]
            .iter()
            .cloned()
            .chain(std::iter::once(eof))
            .inspect(move |_| counter.set(counter.get() + 1));
        // Let the same DM expression grammar determine where `as`/`in` suffixes
        // begin. Its final operator lookahead is not part of the expression.
        scratch
            .parse_expression(tokens[first].location, stream)
            .ok()?;
        if scratch
            .errors()
            .iter()
            .any(|error| error.severity() == Severity::Error)
        {
            return None;
        }
        let last = tokens.get(first + consumed.get().checked_sub(2)?)?;
        let token_start = relative.offset(last.location)?;
        let end = physical_token_end(bytes, token_start, &last.token)?;
        self.span(whole.start + start, whole.start + end)
    }

    fn macro_expression_span(&self, location: Location, define: &Define) -> Option<SourceSpan> {
        let base = self.offset(location)?;
        let bytes = &self.bytes[base..];
        let relative = PhysicalFile::new(String::new(), bytes.to_vec());
        let scratch = Context::default();
        let tokens = dreammaker::Lexer::new(&scratch, Location::BUILTINS.file, bytes)
            .take_while(|token| token.token != Token::Punct(Punctuation::Newline))
            .filter(|token| {
                !matches!(
                    token.token,
                    Token::Punct(Punctuation::Space | Punctuation::Tab) | Token::DocComment(_)
                )
            })
            .collect::<Vec<_>>();
        let mut first = 1; // The first token is the macro name.
        if !define.params.is_empty() {
            if tokens.get(first)?.token != Token::Punct(Punctuation::LParen) {
                return None;
            }
            while tokens.get(first)?.token != Token::Punct(Punctuation::RParen) {
                first += 1;
            }
            first += 1;
        }
        let start = relative.offset(tokens.get(first)?.location)?;
        let last = tokens.last()?;
        let end = physical_token_end(bytes, relative.offset(last.location)?, &last.token)?;
        self.span(base + start, base + end)
    }
}

/// Token kind and location come from DreamMaker's lexer. Only recover the raw
/// final token's physical spelling here, leaving escaped text and comments intact.
fn physical_token_end(bytes: &[u8], start: usize, token: &Token) -> Option<usize> {
    match token {
        Token::Punct(_) => {
            let spelling = token.to_string();
            bytes
                .get(start..start + spelling.len())
                .filter(|value| *value == spelling.as_bytes())
                .map(|_| start + spelling.len())
        }
        Token::Ident(..) => {
            let mut end = start;
            while bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte >= 128)
            {
                end += 1;
            }
            (end > start).then_some(end)
        }
        Token::Int(_) | Token::Float(_) => {
            let mut end = start;
            while bytes.get(end).is_some_and(|byte| {
                byte.is_ascii_alphanumeric()
                    || *byte == b'.'
                    || (matches!(byte, b'+' | b'-')
                        && end > start
                        && matches!(bytes[end - 1], b'e' | b'E'))
            }) {
                end += 1;
            }
            (end > start).then_some(end)
        }
        Token::String(_) | Token::Resource(_) | Token::InterpStringEnd(_) => {
            if bytes.get(start) == Some(&b'@') {
                let (content_start, terminator): (usize, &[u8]) = match bytes.get(start + 1)? {
                    b'(' => {
                        let end =
                            bytes[start + 2..].iter().position(|byte| *byte == b')')? + start + 2;
                        (end + 1, &bytes[start + 2..end])
                    }
                    b'{' if bytes.get(start + 2) == Some(&b'"') => (start + 3, b"\"}"),
                    b'{' => (start + 2, b"{"),
                    _ => (start + 2, &bytes[start + 1..start + 2]),
                };
                if terminator.is_empty() {
                    return None;
                }
                return bytes[content_start..]
                    .windows(terminator.len())
                    .position(|window| window == terminator)
                    .map(|position| content_start + position + terminator.len());
            }
            let (mut position, quote, block) = match bytes.get(start)? {
                b'\'' => (start + 1, b'\'', false),
                b'"' => (start + 1, b'"', false),
                b'{' if bytes.get(start + 1) == Some(&b'"') => (start + 2, b'"', true),
                b']' => (start + 1, b'"', false),
                _ => return None,
            };
            while let Some(byte) = bytes.get(position) {
                if *byte == b'\\' {
                    position += 2;
                    continue;
                }
                if *byte == quote && (!block || bytes.get(position + 1) == Some(&b'}')) {
                    let block_interpolation_end = matches!(token, Token::InterpStringEnd(_))
                        && bytes.get(position + 1) == Some(&b'}');
                    return Some(
                        position
                            + if block || block_interpolation_end {
                                2
                            } else {
                                1
                            },
                    );
                }
                position += 1;
            }
            None
        }
        _ => None,
    }
}

/// Capture and hash each authorized file before the parser opens it, then verify
/// those exact bytes again before publishing. Metadata-based reuse is not used.
#[derive(Debug)]
struct CapturedReads {
    policy: PathPolicy,
    root: PathBuf,
    files: Mutex<BTreeMap<PathBuf, PhysicalFile>>,
}
impl ReadPolicy for CapturedReads {
    fn resolve(&self, path: &Path) -> std::io::Result<PathBuf> {
        let resolved = self.policy.read_path(path).map_err(std::io::Error::other)?;
        let bytes = std::fs::read(&resolved)?;
        let relative = resolved
            .strip_prefix(&self.root)
            .map_err(std::io::Error::other)?;
        let relative = relative
            .to_str()
            .ok_or_else(|| std::io::Error::other("non-Unicode source path"))?
            .replace('\\', "/");
        let mut files = self
            .files
            .lock()
            .map_err(|_| std::io::Error::other("source capture lock poisoned"))?;
        if let Some(previous) = files.get(&resolved) {
            if previous.bytes != bytes {
                return Err(std::io::Error::other("source changed during parsing"));
            }
        } else {
            files.insert(resolved.clone(), PhysicalFile::new(relative, bytes));
        }
        Ok(resolved)
    }
}
impl CapturedReads {
    fn verify(&self) -> Result<()> {
        for (path, file) in self
            .files
            .lock()
            .map_err(|_| anyhow!("source capture lock poisoned"))?
            .iter()
        {
            ensure!(
                self.policy.read_path(path)? == *path && std::fs::read(path)? == file.bytes,
                "source changed during authoring export: {}",
                file.path
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Occurrence {
    owner: String,
    whole: SourceSpan,
    initializer: Option<SourceSpan>,
}

struct SourceIndex<'a> {
    files_by_id: BTreeMap<dreammaker::FileId, &'a PhysicalFile>,
    files_by_name: BTreeMap<&'a str, &'a PhysicalFile>,
    fields: BTreeMap<(String, String), Vec<Occurrence>>,
    headers: BTreeMap<String, Vec<SourceSpan>>,
    macros: Vec<SourceSpan>,
    scopes: BTreeMap<String, Vec<(SourceSpan, String)>>,
}

impl<'a> SourceIndex<'a> {
    fn file(&self, location: Location) -> Option<&PhysicalFile> {
        self.files_by_id.get(&location.file).copied()
    }
    fn by_span(&self, span: &SourceSpan) -> &PhysicalFile {
        self.files_by_name
            .get(span.path.as_str())
            .copied()
            .expect("source spans belong to captured inputs")
    }
    fn annotation_span(&self, start: Location, end_inclusive: Location) -> Option<SourceSpan> {
        let file = self.file(start)?;
        let end = if start.file != end_inclusive.file {
            // An include's EOF terminator may be attributed to its includer.
            // Physical expressions are independently parsed below before editing.
            file.bytes.len()
        } else if end_inclusive.column == u16::MAX {
            file.offset(end_inclusive)?
        } else {
            file.offset(end_inclusive)?
                .checked_add(1)?
                .min(file.bytes.len())
        };
        file.trim_span(file.offset(start)?, end)
    }
    fn build(
        context: &'a Context,
        root: &'a Path,
        files: &'a BTreeMap<PathBuf, PhysicalFile>,
        tree: &ObjectTree,
        annotations: &AnnotationTree,
    ) -> Self {
        let mut files_by_id = BTreeMap::new();
        context.file_list().for_each(|reported| {
            let path = if reported.is_absolute() {
                reported.to_path_buf()
            } else {
                root.join(reported)
            };
            if let (Some(id), Ok(path)) = (context.get_file(reported), path.canonicalize()) {
                if let Some(file) = files.get(&path) {
                    files_by_id.insert(id, file);
                }
            }
        });
        let mut index = Self {
            files_by_id,
            files_by_name: files
                .values()
                .map(|file| (file.path.as_str(), file))
                .collect(),
            fields: BTreeMap::new(),
            headers: BTreeMap::new(),
            macros: Vec::new(),
            scopes: BTreeMap::new(),
        };
        for (range, annotation) in annotations.iter() {
            let Some(span) = index.annotation_span(range.start, range.end) else {
                continue;
            };
            match annotation {
                Annotation::MacroUse { .. } => index.macros.push(span),
                Annotation::Variable(parts) => {
                    let Some((owner, name)) = variable_owner(tree, parts) else {
                        continue;
                    };
                    let file = index.by_span(&span);
                    let initializer = file.initializer_span(&span);
                    index
                        .fields
                        .entry((owner.clone(), name))
                        .or_default()
                        .push(Occurrence {
                            owner,
                            whole: span,
                            initializer,
                        });
                }
                Annotation::TreePath(absolute, parts) if *absolute => {
                    let path = format!(
                        "/{}",
                        parts
                            .iter()
                            .map(Ident::as_str)
                            .collect::<Vec<_>>()
                            .join("/")
                    );
                    if tree.find(&path).is_some() && index.by_span(&span).text(&span) == path {
                        index.headers.entry(path).or_default().push(span);
                    }
                }
                _ => {}
            }
        }
        // A relative type header still has a semantic declaration location.
        for ty in tree.iter_types() {
            if let Some(span) = index
                .file(ty.location)
                .and_then(|file| file.token_span(ty.location))
            {
                let headers = index.headers.entry(ty.path.clone()).or_default();
                if !headers.contains(&span) {
                    headers.push(span);
                }
            }
        }
        for values in index.fields.values_mut() {
            values.sort_by(|a, b| a.whole.cmp(&b.whole));
        }
        for headers in index.headers.values_mut() {
            headers.sort();
            headers.dedup();
        }
        index.macros.sort();
        index.macros.dedup();
        for occurrence in index.fields.values().flatten() {
            index
                .scopes
                .entry(occurrence.whole.path.clone())
                .or_default()
                .push((occurrence.whole.clone(), occurrence.owner.clone()));
        }
        for ty in tree.iter_types() {
            for proc_ref in ty.iter_self_procs() {
                if let Some(span) = index.procedure_span(proc_ref.get()) {
                    index
                        .scopes
                        .entry(span.path.clone())
                        .or_default()
                        .push((span, ty.path.clone()));
                }
            }
        }
        for scopes in index.scopes.values_mut() {
            scopes.sort_by(|a, b| a.0.cmp(&b.0));
        }
        index
    }
    fn overlaps_macro(&self, span: &SourceSpan) -> bool {
        self.macros
            .iter()
            .any(|mac| mac.path == span.path && mac.start < span.end && span.start < mac.end)
    }
    fn field(&self, ty: TypeRef<'_>, name: &str) -> Field {
        let owner = ty
            .iter_parent_types()
            .find(|parent| parent.vars.contains_key(name))
            .expect("field owner exists");
        let value = &owner.vars[name].value;
        let key = (owner.path.clone(), name.to_owned());
        let occurrences = self.fields.get(&key).map(Vec::as_slice).unwrap_or_default();
        let value_offset = self
            .file(value.location)
            .and_then(|file| file.offset(value.location));
        let value_path = self.file(value.location).map(|file| &file.path);
        let effective = occurrences.iter().find(|occurrence| {
            value_path == Some(&occurrence.whole.path)
                && value_offset.is_some_and(|offset| {
                    occurrence.whole.start <= offset && offset <= occurrence.whole.end
                })
        });
        let source = effective.and_then(|occurrence| occurrence.initializer.clone());
        let expression = source.as_ref().map(|span| self.by_span(span).text(span));
        let editable = source.as_ref().is_some_and(|span| self.by_span(span).utf8);
        Field {
            name: name.to_owned(),
            expression,
            value: value
                .constant
                .as_ref()
                .map(constant_json)
                .unwrap_or(Value::Null),
            value_known: value.constant.is_some(),
            owner_type: owner.path.clone(),
            source,
            editable,
            local: owner.path == ty.path,
            occurrences: occurrences
                .iter()
                .map(|occurrence| FieldOccurrence {
                    source: occurrence.initializer.clone(),
                    expression: occurrence
                        .initializer
                        .as_ref()
                        .map(|span| self.by_span(span).text(span)),
                    editable: occurrence
                        .initializer
                        .as_ref()
                        .is_some_and(|span| self.by_span(span).utf8),
                })
                .collect(),
        }
    }
    fn procedure_span(&self, value: &dreammaker::objtree::ProcValue) -> Option<SourceSpan> {
        let file = self.file(value.header_location)?;
        let start = file.offset(value.header_location)?;
        let body = value.body_range.as_ref()?;
        let end = if body.end.file == value.header_location.file {
            let offset = file.offset(body.end)?;
            match file.bytes.get(offset) {
                Some(b'}' | b';') => offset + 1,
                // Dedents are synthetic; do not consume a sibling's first byte.
                _ => offset,
            }
        } else {
            // Include EOF dedents can be attributed to the including file.
            file.bytes.len()
        };
        file.trim_span(start, end)
    }
    fn owner_at(&self, location: Location) -> Option<String> {
        let file = self.file(location)?;
        let offset = file.offset(location)?;
        let scopes = self.scopes.get(&file.path)?;
        let position = scopes.partition_point(|(span, _)| span.start <= offset);
        let (span, owner) = scopes.get(position.checked_sub(1)?)?;
        (offset < span.end).then(|| owner.clone())
    }
}

fn variable_owner(tree: &ObjectTree, parts: &[Ident]) -> Option<(String, String)> {
    let name = parts.last()?.to_string();
    for length in (0..parts.len()).rev() {
        let owner = format!(
            "/{}",
            parts[..length]
                .iter()
                .map(Ident::as_str)
                .collect::<Vec<_>>()
                .join("/")
        );
        if tree
            .find(&owner)
            .is_some_and(|ty| ty.vars.contains_key(name.as_str()))
        {
            return Some((owner, name));
        }
    }
    None
}

fn arguments_json(arguments: &Arguments) -> Value {
    if arguments.iter().all(|(_, value)| value.is_none()) {
        Value::Array(
            arguments
                .iter()
                .map(|(key, _)| constant_json(key))
                .collect(),
        )
    } else {
        json!({"entries": arguments.iter().map(|(key, value)| json!({"key": constant_json(key), "value": value.as_ref().map(constant_json)})).collect::<Vec<_>>()})
    }
}
fn constant_json(constant: &Constant) -> Value {
    match constant {
        Constant::Null(_) => Value::Null,
        Constant::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && f64::from(*value) >= i64::MIN as f64
                && f64::from(*value) < i64::MAX as f64 =>
        {
            json!(*value as i64)
        }
        Constant::Float(value) if !value.is_finite() => {
            json!({"kind":"non_finite", "value":value.to_string()})
        }
        Constant::Float(value) => json!(value),
        Constant::String(value) | Constant::Resource(value) => json!(value.as_str()),
        Constant::List(arguments) => arguments_json(arguments),
        Constant::Prefab(prefab) if prefab.vars.is_empty() => json!(prefab.path.to_string()),
        Constant::Prefab(prefab) => {
            json!({"kind":"prefab", "type_path":prefab.path.to_string(), "vars":prefab.vars.iter().map(|(name, value)| (name.to_string(), constant_json(value))).collect::<BTreeMap<_, _>>()})
        }
        Constant::New { type_, args } => {
            json!({"kind":"new", "type":type_.as_ref().map(|value| json!({"type_path":value.path.to_string(),"vars":value.vars.iter().map(|(name,value)| (name.to_string(),constant_json(value))).collect::<BTreeMap<_,_>>()})), "args":args.as_ref().map(arguments_json)})
        }
        Constant::Call(function, arguments) => {
            json!({"kind":"constructor", "name":format!("{function:?}"), "args":arguments_json(arguments)})
        }
    }
}

fn kind(ty: TypeRef<'_>) -> &'static str {
    for parent in ty.iter_parent_types() {
        match parent.path.as_str() {
            "/datum/job" => return "job",
            "/datum/outfit" => return "outfit",
            "/datum/id_trim" => return "id_trim",
            "/obj/item" => return "item",
            _ => {}
        }
    }
    "related"
}

const ITEM_FIELDS: &[&str] = &[
    "name",
    "desc",
    "icon",
    "icon_state",
    "worn_icon",
    "worn_icon_state",
    "lefthand_file",
    "righthand_file",
    "inhand_icon_state",
    "slot_flags",
    "body_parts_covered",
    "supports_variations_flags",
    "body_variation_flags",
    "greyscale_config",
    "greyscale_colors",
    "greyscale_config_worn",
    "greyscale_config_inhand_left",
    "greyscale_config_inhand_right",
    "species_exception",
    "species_restricted",
    "sprite_sheets",
    "worn_icon_digitigrade",
];

fn authoring_constant(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ITEM_SLOT_",
        "SLOT_",
        "ITEM_EQUIP_",
        "BODYTYPE_",
        "BODYPART_",
        "BODY_ZONE_",
        "CLOTHING_",
        "SUPPORTS_VARIATIONS_",
        "DIGITIGRADE_",
        "SPECIES_",
        "ACCESS_",
        "REGION_ACCESS_",
        "JOB_",
        "DEPARTMENT_",
        "ACCOUNT_",
        "PAYCHECK_",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        || matches!(
            name,
            "HEAD"
                | "CHEST"
                | "GROIN"
                | "LEGS"
                | "FEET"
                | "ARMS"
                | "HANDS"
                | "EYES"
                | "MOUTH"
                | "FULL_BODY"
                | "UPPER_TORSO"
                | "LOWER_TORSO"
                | "NONE"
                | "TRUE"
                | "FALSE"
                | "DM_VERSION"
                | "DM_BUILD"
                | "SPACEMAN_DMM"
        )
}

struct MacroExpansion<'ctx> {
    context: &'ctx Context,
    preprocessor: Preprocessor<'ctx>,
}

fn macro_depends_on_context(
    name: &str,
    definitions: &BTreeMap<&str, &Define>,
    visited: &mut BTreeSet<String>,
    depth: usize,
) -> bool {
    if matches!(
        name,
        "__FILE__" | "__LINE__" | "__MAIN__" | "__PROC__" | "__TYPE__" | "__IMPLIED_TYPE__"
    ) || depth >= 32
    {
        return true;
    }
    if !visited.insert(name.to_owned()) {
        return false;
    }
    definitions.get(name).is_some_and(|define| {
        define.subst.iter().any(|token| {
            if let Token::Ident(dependency, _) = token {
                macro_depends_on_context(dependency.as_str(), definitions, visited, depth + 1)
            } else {
                false
            }
        })
    })
}

fn macro_constant_value(
    expansion: &mut MacroExpansion<'_>,
    name: &str,
    define: &Define,
) -> Option<Constant> {
    if !define.params.is_empty() || define.variadic || define.subst.is_empty() {
        return None;
    }
    let errors_before = expansion.context.errors().len();
    expansion
        .preprocessor
        .push_file(
            PathBuf::from("<authoring-constant>"),
            std::io::Cursor::new(format!("{name}\n").into_bytes()),
        )
        .ok()?;
    let mut tokens = expansion
        .preprocessor
        .by_ref()
        .filter(|token| {
            !matches!(
                token.token,
                Token::Punct(Punctuation::Space | Punctuation::Tab | Punctuation::Newline)
                    | Token::DocComment(_)
            )
        })
        .peekable();
    let expression = expansion
        .context
        .parse_expression(Location::BUILTINS, &mut tokens)
        .ok()?;
    if tokens.peek().is_some()
        || expansion
            .context
            .errors()
            .iter()
            .skip(errors_before)
            .any(|error| error.severity() == Severity::Error)
    {
        return None;
    }
    expression.simple_evaluate(Location::BUILTINS).ok()
}

fn export_constants(
    history: &DefineHistory,
    index: &SourceIndex<'_>,
    diagnostics: &mut Vec<String>,
    expansion: &mut MacroExpansion<'_>,
) -> (Vec<MacroConstant>, AnalysisConfiguration) {
    let mut active = BTreeMap::new();
    // Finalize stores live definition stacks with INVALID end files and ascending
    // end columns within each stack. Ended/undefined definitions have real ends.
    for (range, (name, define)) in history.iter() {
        if range.end.file == dreammaker::FileId::INVALID {
            let entry =
                active
                    .entry(name.to_string())
                    .or_insert((range.end.column, range.start, define));
            if range.end.column > entry.0 {
                *entry = (range.end.column, range.start, define);
            }
        }
    }
    let when_compile_defined_at_end = active.contains_key("WHEN_COMPILE");
    let definitions = active
        .iter()
        .map(|(name, (_, _, define))| (name.as_str(), *define))
        .collect::<BTreeMap<_, _>>();
    let constants = active
        .iter()
        .filter(|(name, _)| authoring_constant(name))
        .map(|(name, (_, location, define))| {
            let source = index
                .file(*location)
                .and_then(|file| file.macro_expression_span(*location, define));
            let expression = source
                .as_ref()
                .map(|span| index.by_span(span).text(span))
                .unwrap_or_else(|| {
                    define
                        .subst
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            let value = if macro_depends_on_context(name, &definitions, &mut BTreeSet::new(), 0) {
                None
            } else {
                macro_constant_value(expansion, name, define)
            };
            if value.is_none() {
                diagnostics.push(format!(
                    "{name}: function-like or context-dependent macro value is unsupported."
                ));
            }
            MacroConstant {
                name: name.clone(),
                expression,
                value: value.as_ref().map(constant_json).unwrap_or(Value::Null),
                value_known: value.is_some(),
                source,
            }
        })
        .collect::<Vec<_>>();
    let builtin_defines = constants
        .iter()
        .filter(|constant| {
            matches!(
                constant.name.as_str(),
                "DM_VERSION" | "DM_BUILD" | "SPACEMAN_DMM"
            )
        })
        .map(|constant| (constant.name.clone(), constant.value.clone()))
        .collect();
    let configuration = AnalysisConfiguration {
        profile: "spacemandmm-default",
        command_line_defines: Vec::new(),
        config_file: index
            .files_by_name
            .keys()
            .find(|path| path.eq_ignore_ascii_case("SpacemanDMM.toml"))
            .map(|path| (*path).to_owned()),
        builtin_defines,
        when_compile_defined_at_end,
    };
    (constants, configuration)
}

fn export_tree(
    context: &Context,
    root: &Path,
    files: &BTreeMap<PathBuf, PhysicalFile>,
    tree: &ObjectTree,
    annotations: &AnnotationTree,
    history: &DefineHistory,
    expansion: &mut MacroExpansion<'_>,
) -> Result<AuthoringExport> {
    let index = SourceIndex::build(context, root, files, tree, annotations);
    let table = ReferenceTable::build(tree);
    let mut selected = tree
        .iter_types()
        .filter(|ty| kind(*ty) != "related")
        .map(|ty| ty.path.clone())
        .collect::<BTreeSet<_>>();
    for path in selected.clone() {
        if let Some(ty) = tree.find(&path) {
            for parent in ty.iter_parent_types() {
                if !parent.path.is_empty() {
                    selected.insert(parent.path.clone());
                }
            }
        }
    }
    let mut diagnostics = vec!["Static references do not cover dynamic/string paths, maps, inactive preprocessor branches, or runtime hook effects.".to_owned()];
    diagnostics.push("Analysis profile uses SpacemanDMM defaults without maintained-build command-line defines; WHEN_COMPILE/BYOND validation is separate.".to_owned());
    let (constants, configuration) = export_constants(history, &index, &mut diagnostics, expansion);
    if table.skipped_dynamic() != 0 {
        diagnostics.push(format!(
            "{} dynamic references were not resolved (environment scope).",
            table.skipped_dynamic()
        ));
    }
    if !index.macros.is_empty() {
        diagnostics.push("macro-expanded reference migration is unsupported; physical macro initializer expressions are preserved.".to_owned());
    }
    for file in files.values().filter(|file| !file.utf8) {
        diagnostics.push(format!(
            "{}: Latin-1 input is inspectable but source edits are disabled.",
            file.path
        ));
    }
    let mut references: BTreeMap<String, Vec<Reference>> = BTreeMap::new();
    for target in tree
        .iter_types()
        .filter(|ty| matches!(kind(*ty), "job" | "outfit" | "id_trim"))
    {
        for hit in table
            .references(target.id)
            .iter()
            .filter(|hit| hit.kind == ReferenceKind::TypePath)
        {
            let Some(owner) = index.owner_at(hit.location) else {
                continue;
            };
            let Some(file) = index.file(hit.location) else {
                continue;
            };
            let Some(mut source) = file.token_span(hit.location) else {
                continue;
            };
            if let Some(mac) = index.macros.iter().find(|mac| {
                mac.path == source.path && mac.start <= source.start && source.start < mac.end
            }) {
                source = mac.clone();
            }
            let text = file.text(&source);
            let macro_expanded = index.overlaps_macro(&source);
            // The reference table has resolved this exact location to target.id;
            // require an exact physical literal as a second, independent check.
            let editable =
                file.utf8 && !macro_expanded && text.starts_with('/') && text == target.path;
            let reason = if macro_expanded {
                Some("macro expansion has no independently editable physical type path".to_owned())
            } else if !editable {
                Some("implicit, relative, or unverified physical type path".to_owned())
            } else {
                None
            };
            let identity = format!(
                "{}:{}:{}:{}",
                source.path, source.start, source.end, target.path
            );
            let reference = Reference {
                id: format!("ref:{:x}", Sha256::digest(identity)),
                target_type: target.path.clone(),
                source,
                editable,
                reason,
            };
            selected.insert(owner.clone());
            references.entry(owner).or_default().push(reference);
            selected.insert(target.path.clone());
        }
    }
    let mut definitions = Vec::new();
    for path in selected {
        let Some(ty) = tree.find(&path) else {
            continue;
        };
        let category = kind(ty);
        let mut names = BTreeSet::new();
        // The parser root holds globals, which are not inherited instance members.
        match category {
            "item" => names.extend(
                ITEM_FIELDS
                    .iter()
                    .filter(|name| {
                        ty.iter_parent_types().any(|parent| {
                            !parent.path.is_empty() && parent.vars.contains_key(**name)
                        })
                    })
                    .map(|name| (*name).to_owned()),
            ),
            "related" => names.extend(ty.vars.keys().map(ToString::to_string)),
            _ => {
                for parent in ty
                    .iter_parent_types()
                    .filter(|parent| !parent.path.is_empty())
                {
                    names.extend(parent.vars.keys().map(ToString::to_string));
                }
            }
        }
        let fields = names.iter().map(|name| index.field(ty, name)).collect();
        let mut procedure_names = BTreeSet::new();
        // Item selectors need identity and appearance, not duplicate every
        // inherited runtime procedure for thousands of item types.
        if category != "item" && category != "related" {
            for parent in ty
                .iter_parent_types()
                .filter(|parent| !parent.path.is_empty())
            {
                procedure_names.extend(parent.procs.keys().map(ToString::to_string));
            }
        }
        let mut procedures = Vec::new();
        for name in procedure_names {
            let owner = ty
                .iter_parent_types()
                .find(|parent| parent.procs.contains_key(name.as_str()))
                .expect("procedure owner exists");
            for proc_ref in owner
                .iter_self_procs()
                .filter(|proc_ref| proc_ref.name() == name)
            {
                if proc_ref.location.is_builtins() {
                    continue;
                }
                let source = index.procedure_span(proc_ref.get());
                let text = source
                    .as_ref()
                    .map(|span| index.by_span(span).text(span))
                    .unwrap_or_default();
                let editable = source.as_ref().is_some_and(|span| index.by_span(span).utf8)
                    && proc_ref.code.is_some();
                procedures.push(Procedure {
                    name: name.clone(),
                    owner_type: owner.path.clone(),
                    source,
                    text,
                    editable,
                    override_index: proc_ref.index(),
                    local: owner.path == ty.path,
                });
            }
        }
        let occurrences = index.headers.get(&path).cloned().unwrap_or_default();
        let own_location = index
            .file(ty.location)
            .and_then(|file| file.offset(ty.location).map(|offset| (&file.path, offset)));
        let source = occurrences
            .iter()
            .find(|span| {
                own_location.is_some_and(|(path, offset)| {
                    span.path == *path && span.start <= offset && offset < span.end
                })
            })
            .cloned();
        let mut refs = references.remove(&path).unwrap_or_default();
        refs.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then(a.target_type.cmp(&b.target_type))
        });
        refs.dedup_by(|a, b| a.id == b.id);
        definitions.push(Definition {
            type_path: path,
            parent_type: ty.parent_type().map(|parent| parent.path.clone()),
            kind: category,
            source,
            occurrences,
            fields,
            procedures,
            references: refs,
        });
    }
    diagnostics.extend(
        context
            .errors()
            .iter()
            .filter(|error| !error.location().is_builtins())
            .map(|error| {
                format!(
                    "{}:{}:{}: {}",
                    index
                        .file(error.location())
                        .map(|file| file.path.as_str())
                        .unwrap_or("<parser>"),
                    error.location().line,
                    error.location().column,
                    error.description()
                )
            }),
    );
    diagnostics.sort();
    diagnostics.dedup();
    Ok(AuthoringExport {
        schema_version: 1,
        analyzer_version: format!("meridian-mcp/{}/authoring-v1", env!("CARGO_PKG_VERSION")),
        spacemandmm_revision: crate::capabilities::SPACEMANDMM_REVISION,
        spacemandmm_local_patch_sha256: crate::capabilities::SPACEMANDMM_LOCAL_PATCH_SHA256,
        environment: "tgstation.dme".to_owned(),
        configuration,
        input_files: files
            .values()
            .map(|file| InputFile {
                path: file.path.clone(),
                sha256: file.sha256.clone(),
                encoding: if file.utf8 { "utf-8" } else { "latin-1" },
            })
            .collect(),
        definitions,
        constants,
        diagnostics,
    })
}

pub fn export_project(project: &Path, output: &Path) -> Result<()> {
    let root = project
        .canonicalize()
        .context("cannot resolve authoring project")?;
    ensure!(root.is_dir(), "authoring project must be a directory");
    let policy = PathPolicy::new(vec![root.clone()], vec![])?;
    let environment = policy.read_path(root.join("tgstation.dme"))?;
    ensure!(
        output
            .extension()
            .is_some_and(|extension| extension == "json"),
        "authoring output must be a .json file"
    );
    let reads = Arc::new(CapturedReads {
        policy,
        root: root.clone(),
        files: Mutex::new(BTreeMap::new()),
    });
    let mut context = Context::default();
    context.set_read_policy(reads.clone());
    let config_path = root.join("SpacemanDMM.toml");
    let config_present = config_path.exists();
    context.autodetect_config(&environment);
    let mut preprocessor = Preprocessor::new(&context, environment)?;
    preprocessor.enable_annotations();
    let mut annotations = AnnotationTree::default();
    let (fatal, tree) = {
        let mut parser = Parser::new(&context, &mut preprocessor);
        parser.enable_procs();
        parser.annotate_to(&mut annotations);
        parser.parse_object_tree_2()
    };
    if let Some(preprocessor_annotations) = preprocessor.take_annotations() {
        annotations.merge(preprocessor_annotations);
    }
    let macro_context = Context::default();
    let mut expansion = MacroExpansion {
        context: &macro_context,
        preprocessor: preprocessor.branch_with_current_defines(&macro_context),
    };
    let history = preprocessor.finalize();
    ensure!(
        !context.read_denied(),
        "authoring parse denied an input outside the project or detected source drift"
    );
    ensure!(
        !tree.parent_cycle_detected(),
        "authoring parse has cyclic parent_type inheritance"
    );
    let errors = context
        .errors()
        .iter()
        .filter(|error| error.severity() == Severity::Error)
        .map(|error| error.description().to_owned())
        .collect::<Vec<_>>();
    ensure!(
        !fatal && errors.is_empty(),
        "authoring parse failed: {}",
        errors.join("; ")
    );
    reads.verify()?;
    ensure!(
        config_path.exists() == config_present,
        "parser configuration presence changed during authoring export"
    );
    let files = reads
        .files
        .lock()
        .map_err(|_| anyhow!("source capture lock poisoned"))?;
    let export = export_tree(
        &context,
        &root,
        &files,
        &tree,
        &annotations,
        &history,
        &mut expansion,
    )?;
    let bytes = serde_json::to_vec(&export)?;
    let output_parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output_policy = PathPolicy::new(vec![output_parent.to_owned()], vec![])?;
    let resolved_output = output_policy.output_path(output, true)?;
    ensure!(
        !files.contains_key(&resolved_output),
        "authoring output would overwrite a parsed source input"
    );
    drop(files);
    reads.verify()?;
    write_atomic(&output_policy, &resolved_output, true, |file| {
        file.write_all(&bytes).map_err(AtomicOutputError::from)
    })?;
    Ok(())
}

/// Returns false for ordinary MCP startup, preserving the existing no-argument
/// executable contract. Export requires no Codex registration or ambient roots.
pub fn dispatch_cli(arguments: &[std::ffi::OsString]) -> Result<bool> {
    if arguments
        .first()
        .is_none_or(|value| value != "authoring-export")
    {
        return Ok(false);
    }
    let mut project = None;
    let mut output = None;
    let mut position = 1;
    while position < arguments.len() {
        let name = &arguments[position];
        let value = arguments
            .get(position + 1)
            .ok_or_else(|| anyhow!("missing authoring-export argument value"))?;
        match name.to_str() {
            Some("--project") if project.is_none() => project = Some(PathBuf::from(value)),
            Some("--output") if output.is_none() => output = Some(PathBuf::from(value)),
            _ => bail!("expected authoring-export --project <game-root> --output <json-file>"),
        }
        position += 2;
    }
    export_project(
        &project.ok_or_else(|| anyhow!("authoring-export requires --project"))?,
        &output.ok_or_else(|| anyhow!("authoring-export requires --output"))?,
    )?;
    Ok(true)
}
