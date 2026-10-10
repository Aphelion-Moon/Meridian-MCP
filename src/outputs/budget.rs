use crate::outputs::{Omissions, SourceFields};
use crate::source::SourceExcerpt;
use dreammaker::constants::{Constant, Pop};
use std::fmt::Write;

pub(crate) const DETAIL_BYTES: usize = 128 * 1024;
pub(crate) const DOC_BYTES: usize = 8 * 1024;
pub(crate) const CONSTANT_BYTES: usize = 4 * 1024;
pub(crate) const MEMBER_WORK: usize = 10_000;

pub(crate) struct Budget {
    pub bytes: usize,
    pub source_lines: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: DETAIL_BYTES,
            source_lines: crate::source::MAX_SOURCE_LINES,
        }
    }
}
impl Budget {
    pub fn text(
        &mut self,
        value: &str,
        maximum: usize,
        field: &str,
        omissions: &mut Omissions,
    ) -> String {
        let retained = crate::result::bounded_text(
            value,
            maximum.min(self.bytes),
            maximum.min(self.bytes),
            false,
        );
        self.bytes = self
            .bytes
            .saturating_sub(crate::result::encoded_bytes(&retained, usize::MAX).unwrap_or(0));
        if retained.len() != value.len() {
            omissions
                .fields
                .insert(field.into(), "UTF-8/encoded aggregate byte limit".into());
        }
        retained.to_owned()
    }
    pub fn document(
        &mut self,
        text: Option<&str>,
        field: &str,
        omissions: &mut Omissions,
    ) -> String {
        match text {
            Some(text) => self.text(text, DOC_BYTES, field, omissions),
            None => {
                omissions.fields.insert(
                    field.into(),
                    "documentation unavailable in the captured index".into(),
                );
                String::new()
            }
        }
    }
    pub fn source(&mut self, source: Option<&SourceExcerpt>, maximum: usize) -> SourceFields {
        let mut fields = SourceFields {
            source: Some(None),
            source_start_line: Some(source.map(|v| v.start_line)),
            source_start_column: Some(source.map(|v| v.start_column)),
            source_total_lines: Some(source.map(|v| v.total_lines)),
            source_truncated: Some(source.map(|_| false)),
            source_boundary: Some(source.map(|v| v.boundary)),
            ..Default::default()
        };
        if let Some(source) = source {
            let lines = maximum.min(self.source_lines);
            let mut retained = String::new();
            let mut count = 0;
            let mut clipped = false;
            for line in source.text.split('\n').take(lines) {
                if self.bytes <= 2 {
                    clipped = true;
                    break;
                }
                if count > 0 {
                    retained.push('\n');
                    self.bytes = self.bytes.saturating_sub(2);
                }
                let available = self.bytes.min(32 * 1024);
                let text = crate::result::bounded_text(line, available, available, false);
                self.bytes = self
                    .bytes
                    .saturating_sub(crate::result::encoded_bytes(&text, usize::MAX).unwrap_or(0));
                retained.push_str(text);
                count += 1;
                if text.len() != line.len() {
                    clipped = true;
                    break;
                }
            }
            self.source_lines = self.source_lines.saturating_sub(count);
            fields.source = Some(Some(retained));
            fields.source_truncated = Some(Some(
                clipped || source.truncated(lines) || count < source.total_lines,
            ));
        }
        fields
    }
    pub fn constant(
        &mut self,
        constant: Option<&Constant>,
        field: &str,
        omissions: &mut Omissions,
    ) -> Option<String> {
        let constant = constant?;
        let mut nodes = 512;
        let mut bytes = CONSTANT_BYTES.min(self.bytes);
        if !constant_fits(constant, 16, &mut nodes, &mut bytes) {
            omissions
                .fields
                .insert(field.into(), "constant node/depth/byte limit".into());
            return Some("<constant preview omitted>".into());
        }
        struct Preview {
            text: String,
            maximum: usize,
        }
        impl std::fmt::Write for Preview {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                if self.text.len().saturating_add(text.len()) > self.maximum {
                    return Err(std::fmt::Error);
                }
                self.text.push_str(text);
                Ok(())
            }
        }
        let mut preview = Preview {
            text: String::new(),
            maximum: CONSTANT_BYTES.min(self.bytes),
        };
        if write!(&mut preview, "{constant:?}").is_err() {
            omissions
                .fields
                .insert(field.into(), "constant encoded preview limit".into());
        }
        Some(self.text(&preview.text, CONSTANT_BYTES, field, omissions))
    }
}
fn charge(text: &str, bytes: &mut usize) -> bool {
    match bytes.checked_sub(text.len()) {
        Some(left) => {
            *bytes = left;
            true
        }
        None => false,
    }
}
fn pop_fits(pop: &Pop, depth: usize, nodes: &mut usize, bytes: &mut usize) -> bool {
    pop.path.iter().all(|item| charge(item, bytes))
        && pop
            .vars
            .iter()
            .all(|(name, value)| charge(name, bytes) && constant_fits(value, depth, nodes, bytes))
}
fn constant_fits(value: &Constant, depth: usize, nodes: &mut usize, bytes: &mut usize) -> bool {
    if depth == 0 || *nodes == 0 {
        return false;
    }
    *nodes -= 1;
    let args = match value {
        Constant::Null(path) => {
            return path
                .as_ref()
                .is_none_or(|path| path.iter().all(|item| charge(item, bytes)))
        }
        Constant::Float(_) => return true,
        Constant::String(text) | Constant::Resource(text) => return charge(text, bytes),
        Constant::Prefab(pop) => return pop_fits(pop, depth - 1, nodes, bytes),
        Constant::New { type_, args } => {
            if !type_
                .as_ref()
                .is_none_or(|pop| pop_fits(pop, depth - 1, nodes, bytes))
            {
                return false;
            }
            let Some(args) = args else { return true };
            args
        }
        Constant::List(args) | Constant::Call(_, args) => args,
    };
    args.iter().all(|(key, value)| {
        constant_fits(key, depth - 1, nodes, bytes)
            && value
                .as_ref()
                .is_none_or(|value| constant_fits(value, depth - 1, nodes, bytes))
    })
}
