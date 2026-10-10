use dreammaker::lexer::from_utf8_or_latin1_borrowed;
use dreammaker::Location;

pub(crate) const MAX_SOURCE_LINES: usize = 200;
pub(crate) const DEFAULT_PROC_SOURCE_LINES: usize = 80;

/// Decode physical lines independently, as source excerpts do, and normalize
/// CRLF to LF. Token search retains a leading BOM and lone CR bytes as text.
pub(crate) fn normalize_source_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.replace("\r\n", "\n");
    }
    let mut text = String::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if let Some(line) = line.strip_suffix(b"\n") {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            text.push_str(&from_utf8_or_latin1_borrowed(line));
            text.push('\n');
        } else {
            text.push_str(&from_utf8_or_latin1_borrowed(line));
        }
    }
    text
}

/// Bounded physical source from the same parse snapshot as its symbol metadata.
#[derive(Clone, Debug)]
pub(crate) struct SourceExcerpt {
    pub(crate) text: String,
    pub(crate) start_line: u32,
    pub(crate) start_column: u16,
    pub(crate) total_lines: usize,
    pub(crate) boundary: &'static str,
}

impl SourceExcerpt {
    pub(crate) fn line(text: String, start_line: u32) -> Self {
        Self {
            text,
            start_line,
            start_column: 1,
            total_lines: 1,
            boundary: "declaration_line",
        }
    }

    #[cfg(test)]
    pub(crate) fn render(&self, max_lines: usize) -> String {
        self.text
            .split('\n')
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn truncated(&self, max_lines: usize) -> bool {
        self.total_lines > self.text.split('\n').take(max_lines).count()
    }
}

pub(crate) struct IndexedSource {
    // Parser columns count original bytes. Keep those offsets until decoding an
    // excerpt, including for Latin-1 source and non-ASCII same-line declarations.
    bytes: Vec<u8>,
    line_starts: Vec<usize>,
}

impl IndexedSource {
    pub(crate) fn new(mut bytes: Vec<u8>) -> Self {
        if bytes.starts_with(b"\xef\xbb\xbf") {
            bytes.drain(..3);
        }
        let mut line_starts = Vec::new();
        if !bytes.is_empty() {
            line_starts.push(0);
            line_starts.extend(bytes.iter().enumerate().filter_map(|(index, byte)| {
                (*byte == b'\n' && index + 1 < bytes.len()).then_some(index + 1)
            }));
        }
        Self { bytes, line_starts }
    }

    pub(crate) fn read(path: &std::path::Path) -> std::io::Result<Self> {
        std::fs::read(path).map(Self::new)
    }

    fn line_bytes(&self, one_based_line: u32) -> Option<&[u8]> {
        let index = one_based_line.checked_sub(1)? as usize;
        let start = *self.line_starts.get(index)?;
        let end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        let line = self.bytes.get(start..end)?;
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }

    pub(crate) fn line(&self, one_based_line: u32) -> Option<String> {
        Some(from_utf8_or_latin1_borrowed(self.line_bytes(one_based_line)?).into_owned())
    }

    pub(crate) fn declaration(
        &self,
        start: Location,
        end: Option<Location>,
        max_lines: usize,
    ) -> Option<SourceExcerpt> {
        if max_lines == 0 {
            return None;
        }
        let first = self.line_bytes(start.line)?;
        let start_byte = usize::from(start.column.checked_sub(1)?);
        let prefix = first.get(..start_byte)?;
        let start_byte = if prefix.iter().all(u8::is_ascii_whitespace) {
            0
        } else {
            start_byte
        };
        let (last_line, last_column, boundary) = match end {
            Some(end)
                if end.file == start.file
                    && end.line >= start.line
                    && self.line_bytes(end.line).is_some() =>
            {
                (end.line, Some(end.column), "parser_body_end")
            }
            // An included file's final synthetic dedent can be located in its
            // includer. Report the physical file boundary, not a complete body.
            Some(_) => (
                u32::try_from(self.line_starts.len()).ok()?,
                None,
                "file_end",
            ),
            None => (start.line, None, "declaration_line"),
        };
        let total_lines = (last_line - start.line) as usize + 1;
        let mut lines = Vec::with_capacity(total_lines.min(max_lines));
        for offset in 0..total_lines.min(max_lines) {
            let line_number = start.line + offset as u32;
            let mut line = self.line_bytes(line_number)?;
            if line_number == last_line {
                if let Some(column) = last_column {
                    let token = usize::from(column.saturating_sub(1));
                    // Explicit closing tokens occupy a source byte; synthetic
                    // braces/semicolons mark EOL or the final token at EOF.
                    if matches!(line.get(token), Some(b'}' | b';')) {
                        line = &line[..token + 1];
                    }
                }
            }
            if offset == 0 {
                line = line.get(start_byte..)?;
            }
            lines.push(from_utf8_or_latin1_borrowed(line));
        }
        Some(SourceExcerpt {
            text: lines.join("\n"),
            start_line: start.line,
            start_column: u16::try_from(start_byte + 1).ok()?,
            total_lines,
            boundary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(line: u32, column: u16) -> Location {
        Location {
            line,
            column,
            ..Location::BUILTINS
        }
    }

    #[test]
    fn parser_bounds_keep_comments_and_stop_at_a_nested_sibling() {
        let source = IndexedSource::new(
            b"/datum/test\n\tfirst()\n\t\treturn\n// comment\n\tsecond()\n\t\treturn\n".to_vec(),
        );
        let excerpt = source.declaration(loc(2, 2), Some(loc(4, 11)), 80).unwrap();
        assert_eq!(excerpt.text, "\tfirst()\n\t\treturn\n// comment");
        assert!(!excerpt.truncated(80));
    }

    #[test]
    fn missing_or_invalid_start_never_returns_unrelated_source() {
        let source = IndexedSource::new(b"/proc/one()\n\treturn\n".to_vec());
        for start in [loc(0, 1), loc(99, 1), loc(1, 0), loc(1, 99)] {
            assert!(source.declaration(start, Some(loc(2, 8)), 80).is_none());
        }
        assert!(source.declaration(loc(1, 1), Some(loc(2, 8)), 0).is_none());
    }

    #[test]
    fn line_cap_retains_the_total_span_and_render_limit() {
        let source = IndexedSource::new(b"/proc/one()\n\tvar/one\n\tvar/two\n\treturn\n".to_vec());
        let excerpt = source.declaration(loc(1, 1), Some(loc(4, 8)), 3).unwrap();
        assert_eq!(excerpt.total_lines, 4);
        assert_eq!(excerpt.text.lines().count(), 3);
        assert_eq!(excerpt.render(1), "/proc/one()");
        assert!(excerpt.truncated(200));
    }

    #[test]
    fn file_boundary_is_explicit_when_the_parser_end_leaves_the_file() {
        let source = IndexedSource::new(b"/proc/one()\n\treturn 1".to_vec());
        let excerpt = source.declaration(loc(1, 1), Some(loc(99, 1)), 80).unwrap();
        assert_eq!(excerpt.boundary, "file_end");
        assert_eq!(excerpt.text, "/proc/one()\n\treturn 1");
    }

    #[test]
    fn explicit_brace_ends_before_another_same_line_declaration() {
        let source = IndexedSource::new(b"one() {return;}; two() {return;}".to_vec());
        let first = source.declaration(loc(1, 1), Some(loc(1, 15)), 80).unwrap();
        let second = source
            .declaration(loc(1, 18), Some(loc(1, 32)), 80)
            .unwrap();
        assert_eq!(first.text, "one() {return;}");
        assert_eq!(second.text, "two() {return;}");
        assert_eq!(second.start_column, 18);
    }

    #[test]
    fn bom_and_crlf_follow_parser_byte_locations() {
        let source = IndexedSource::new(b"\xef\xbb\xbf/proc/one()\r\n\treturn 1\r\n".to_vec());
        assert_eq!(source.line(1).as_deref(), Some("/proc/one()"));
        assert_eq!(
            source
                .declaration(loc(1, 1), Some(loc(2, 10)), 80)
                .unwrap()
                .text,
            "/proc/one()\n\treturn 1"
        );
    }

    #[test]
    fn latin1_source_is_decoded_with_the_upstream_rule() {
        let source = IndexedSource::new(b"/proc/one()\n\treturn \"caf\xe9\"".to_vec());
        assert_eq!(
            source
                .declaration(loc(1, 1), Some(loc(2, 9)), 80)
                .unwrap()
                .text,
            "/proc/one()\n\treturn \"caf\u{e9}\""
        );
    }
}
