# Compiler output

`dm_compile` returns build status, artifact identity, provenance, structured diagnostics and the end of each output stream. Large logs cannot replace the build result with a transport size error.

| Option | Default | Accepted values |
| --- | --- | --- |
| `include_output` | `true` | Boolean; `false` omits the `stdout` and `stderr` fields. |
| `output_max_bytes` | 8192 | Integer, 1–65536 UTF-8 bytes per stream. JSON escaping may shorten the returned tail further. |
| `diagnostic_limit` | 50 | Integer, 0–200 rows per severity, also subject to a byte budget. Zero returns counts without rows. |

These options affect the reply only. They do not change compiler execution, success/failure, artifact hashing or provenance recording. Invalid values are rejected before execution. Replies use compact JSON and stay within 512 KiB, below the 1 MiB tool ceiling.

`diagnostic_summary.errors` and `.warnings` count all diagnostics parsed from **captured output**, including rows omitted from the reply. `errors_detail` and `warnings_detail` report returned/omitted rows and shortened messages. An oversized message carries `message_truncated: true` and its original `message_utf8_bytes`. A diagnostic can be omitted by the byte budget even below the requested row limit.

Capture and reply truncation are separate:

- `stdout_truncated_bytes` and `stderr_truncated_bytes` count original bytes discarded by the process capture buffer, which retains at most 512 KiB per stream. If either is nonzero, `diagnostic_summary.capture_complete` is false; counts do not describe the complete compiler log.
- `output_summary` reports available, returned and omitted UTF-8 bytes for each decoded stream. Omitted reply bytes do not include bytes already lost during capture. Disabling output still returns this summary.
- Rare oversized metadata is listed in `response_omissions` by JSON pointer. Arrays retain a prefix with explicit counts. Paths are omitted whole; artifact hashes, existence and provenance status remain available.

Omitted text is not available through a later MCP retrieval call. Use a build workflow that saves full logs when those logs are required. Parser diagnostics from `dm_check_errors` are separate from DreamMaker compiler diagnostics.
