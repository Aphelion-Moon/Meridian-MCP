use super::{diagnostic_to_value, parse_diagnostic_line, response, DiagnosticSeverity};
use crate::process::OutputStream;
use serde_json::{json, Value};

// Bound a pathological unterminated line independently of the total log volume.
// Ordinary multi-line output is analyzed regardless of tail-buffer eviction.
const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct DiagnosticRows {
    total: u64,
    rows: Vec<Value>,
    bytes: usize,
    full: bool,
}

#[derive(Default)]
struct StreamDiagnostics {
    line: Vec<u8>,
    oversized: bool,
    oversized_lines: u64,
    errors: DiagnosticRows,
    warnings: DiagnosticRows,
}

impl StreamDiagnostics {
    fn push(&mut self, bytes: &[u8], limit: usize) {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let part = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            if !self.oversized {
                if self.line.len() + part.len() > MAX_LINE_BYTES {
                    self.oversized = true;
                    self.line.clear();
                } else {
                    self.line.extend_from_slice(part);
                }
            }
            if newline {
                self.finish_line(limit);
            }
        }
    }

    fn finish_line(&mut self, limit: usize) {
        if self.oversized {
            self.oversized_lines += 1;
        } else {
            let text = String::from_utf8_lossy(&self.line);
            if let Some(diagnostic) = parse_diagnostic_line(&text) {
                let target = match diagnostic.severity {
                    DiagnosticSeverity::Error => &mut self.errors,
                    DiagnosticSeverity::Warning => &mut self.warnings,
                };
                target.total += 1;
                if !target.full && target.rows.len() < limit {
                    let row = response::bound_diagnostic(diagnostic_to_value(&diagnostic));
                    let bytes = response::json_bytes(&row) + 1;
                    if target.bytes + bytes + 2 <= response::DIAGNOSTIC_JSON_BYTES {
                        target.bytes += bytes;
                        target.rows.push(row);
                    } else {
                        target.full = true;
                    }
                }
            }
        }
        self.line.clear();
        self.oversized = false;
    }
}

pub(super) struct CompilerDiagnostics {
    stdout: StreamDiagnostics,
    stderr: StreamDiagnostics,
    limit: usize,
}

pub(super) struct DiagnosticResult {
    pub errors: Vec<Value>,
    pub warnings: Vec<Value>,
    pub error_count: u64,
    pub complete: bool,
    pub summary: Value,
}

impl CompilerDiagnostics {
    pub fn new(limit: usize) -> Self {
        Self {
            stdout: StreamDiagnostics::default(),
            stderr: StreamDiagnostics::default(),
            limit,
        }
    }

    pub fn observe(&mut self, stream: OutputStream, bytes: &[u8]) {
        match stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        }
        .push(bytes, self.limit);
    }

    pub fn finish(mut self, output_complete: bool, capture_complete: bool) -> DiagnosticResult {
        self.stdout.finish_line(self.limit);
        self.stderr.finish_line(self.limit);
        let errors = self.stdout.errors.total + self.stderr.errors.total;
        let warnings = self.stdout.warnings.total + self.stderr.warnings.total;
        let oversized_lines = self.stdout.oversized_lines + self.stderr.oversized_lines;
        let complete = output_complete && oversized_lines == 0;
        DiagnosticResult {
            errors: self
                .stdout
                .errors
                .rows
                .into_iter()
                .chain(self.stderr.errors.rows)
                .collect(),
            warnings: self
                .stdout
                .warnings
                .rows
                .into_iter()
                .chain(self.stderr.warnings.rows)
                .collect(),
            error_count: errors,
            complete,
            summary: json!({"scope":"observed_output", "errors":errors, "warnings":warnings,
                "analysis_complete":complete, "output_complete":output_complete,
                "capture_complete":capture_complete, "oversized_lines":oversized_lines}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_splits_crlf_and_unterminated_lines_preserve_diagnostics_and_stream_order() {
        let stdout = "source🛰.dm:7:3: error: first 🛰\r\nsource.dm:8: warning: caution\nsource.dm:9: error: final";
        let stderr = "other.dm:10: error: stderr";
        let mut collector = CompilerDiagnostics::new(200);
        // Feed stderr first; results still match the established stdout-then-stderr order.
        for byte in stderr.bytes() {
            collector.observe(OutputStream::Stderr, &[byte]);
        }
        for byte in stdout.bytes() {
            collector.observe(OutputStream::Stdout, &[byte]);
        }
        let result = collector.finish(true, true);
        assert!(result.complete);
        assert_eq!(result.error_count, 3);
        assert_eq!(result.summary["warnings"], 1);
        assert_eq!(
            result
                .errors
                .iter()
                .map(|r| r["line"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![7, 9, 10]
        );
        assert_eq!(result.errors[0]["file"], "source🛰.dm");
        assert_eq!(result.errors[0]["message"], "first 🛰");
        assert_eq!(result.errors[0]["column"], 3);
    }

    #[test]
    fn oversized_lines_and_incomplete_reads_do_not_discard_other_observed_errors() {
        let mut collector = CompilerDiagnostics::new(2);
        collector.observe(OutputStream::Stdout, &vec![b'x'; MAX_LINE_BYTES + 1]);
        collector.observe(OutputStream::Stdout, b"\nfixture.dm:8: error: retained\n");
        let result = collector.finish(true, false);
        assert!(!result.complete);
        assert_eq!(result.summary["oversized_lines"], 1);
        assert_eq!(result.error_count, 1);
        assert_eq!(result.errors[0]["line"], 8);
        let mut collector = CompilerDiagnostics::new(0);
        collector.observe(
            OutputStream::Stderr,
            b"fixture.dm:9: error: before failure\n",
        );
        let result = collector.finish(false, true);
        assert!(!result.complete);
        assert_eq!(result.error_count, 1);
        assert!(result.errors.is_empty());
    }
}
