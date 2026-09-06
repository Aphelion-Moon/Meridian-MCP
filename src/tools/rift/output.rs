use super::{
    diagnostic_regex, dm_cache_marker, explicit_wrapper_failure, parse_rift_result,
    RiftResultRecord,
};
use crate::process::{output::BoundedLines, OutputStream};
use crate::tools::build_response as response;
use serde_json::{json, Value};

#[derive(Default)]
struct Evidence {
    errors: u64,
    rows: Vec<Value>,
    row_bytes: usize,
    rows_full: bool,
    cache: Option<String>,
    failure: Option<&'static str>,
    record_count: u64,
    record: Option<Result<RiftResultRecord, &'static str>>,
}

impl Evidence {
    fn observe(&mut self, text: &str, limit: usize) {
        if diagnostic_regex().is_match(text) {
            self.errors += 1;
            if !self.rows_full && self.rows.len() < limit {
                let row = response::bound_diagnostic(json!({"message":text.trim()}));
                let bytes = response::json_bytes(&row) + 1;
                if self.row_bytes + bytes + 2 <= response::DIAGNOSTIC_JSON_BYTES {
                    self.row_bytes += bytes;
                    self.rows.push(row);
                } else {
                    self.rows_full = true;
                }
            }
        }
        if self.cache.is_none() {
            self.cache = dm_cache_marker(text);
        }
        if self.failure.is_none() {
            self.failure = explicit_wrapper_failure(text);
        }
        if text.trim().starts_with("RIFT_RESULT ") {
            self.record_count += 1;
            if self.record_count == 1 {
                self.record = Some(
                    parse_rift_result(text)
                        .and_then(|record| record.ok_or("wrapper_result_malformed")),
                );
            }
        }
    }
}

#[derive(Default)]
struct Stream {
    lines: BoundedLines,
    evidence: Evidence,
}

pub(super) struct BuildOutput {
    stdout: Stream,
    stderr: Stream,
    limit: usize,
}

pub(super) struct Analysis {
    pub diagnostics: Vec<Value>,
    pub error_count: u64,
    pub complete: bool,
    pub summary: Value,
    pub cache_evidence: Option<String>,
    pub wrapper_failure: Option<&'static str>,
    pub rift_result: Result<Option<RiftResultRecord>, &'static str>,
}

impl BuildOutput {
    pub fn new(limit: usize) -> Self {
        Self {
            stdout: Stream::default(),
            stderr: Stream::default(),
            limit,
        }
    }

    pub fn observe(&mut self, stream: OutputStream, bytes: &[u8]) {
        let Stream { lines, evidence } = match stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        lines.push(bytes, |text| evidence.observe(text, self.limit));
    }

    pub fn finish(mut self, output_complete: bool, capture_complete: bool) -> Analysis {
        for Stream { lines, evidence } in [&mut self.stdout, &mut self.stderr] {
            lines.finish(|text| evidence.observe(text, self.limit));
        }
        let oversized_lines = self.stdout.lines.oversized_lines + self.stderr.lines.oversized_lines;
        let stdout = self.stdout.evidence;
        let stderr = self.stderr.evidence;
        let errors = stdout.errors + stderr.errors;
        let complete = output_complete && oversized_lines == 0;
        let rift_result = if stdout.record_count + stderr.record_count > 1 {
            Err("wrapper_result_multiple")
        } else {
            stdout.record.or(stderr.record).transpose()
        };
        Analysis {
            diagnostics: stdout.rows.into_iter().chain(stderr.rows).collect(),
            error_count: errors,
            complete,
            summary: json!({"scope":"observed_output", "errors":errors,
                "analysis_complete":complete, "output_complete":output_complete,
                "capture_complete":capture_complete, "oversized_lines":oversized_lines}),
            cache_evidence: stdout.cache.or(stderr.cache),
            wrapper_failure: stdout.failure.or(stderr.failure),
            rift_result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_unicode_crlf_and_final_lines_preserve_stream_order_and_markers() {
        let mut output = BuildOutput::new(2);
        for (stream, text) in [
            (OutputStream::Stderr, "other.dm:9: error: final"),
            (OutputStream::Stdout, "🛰.dm:7:3: error: first 🛰\r\nSkipping 'dm' (up to date)\r\n[offline_preflight_failed]"),
        ] {
            for byte in text.bytes() {
                output.observe(stream, &[byte]);
            }
        }
        let result = output.finish(true, false);
        assert!(result.complete);
        assert_eq!(result.error_count, 2);
        assert_eq!(result.diagnostics[0]["message"], "🛰.dm:7:3: error: first 🛰");
        assert_eq!(result.diagnostics[1]["message"], "other.dm:9: error: final");
        assert_eq!(
            result.cache_evidence.as_deref(),
            Some("Skipping 'dm' (up to date)")
        );
        assert_eq!(result.wrapper_failure, Some("offline_preflight_failed"));
        assert_eq!(result.summary["capture_complete"], false);
    }

    #[test]
    fn records_across_streams_and_incomplete_reads_cannot_be_discarded() {
        let mut output = BuildOutput::new(0);
        output.observe(OutputStream::Stdout, b"RIFT_RESULT {invalid}\n");
        output.observe(OutputStream::Stderr, b"RIFT_RESULT {}\n");
        let result = output.finish(true, true);
        assert_eq!(result.rift_result.unwrap_err(), "wrapper_result_multiple");

        let mut output = BuildOutput::new(0);
        output.observe(
            OutputStream::Stdout,
            &vec![b'x'; crate::process::output::MAX_LINE_BYTES + 1],
        );
        output.observe(OutputStream::Stdout, b"\nfixture.dm:8: error: retained");
        let result = output.finish(false, false);
        assert!(!result.complete);
        assert_eq!(result.summary["oversized_lines"], 1);
        assert_eq!(result.summary["output_complete"], false);
        assert_eq!(result.error_count, 1);
        assert!(result.diagnostics.is_empty());
    }
}
