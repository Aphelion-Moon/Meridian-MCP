# Language-query follow-up protocol

This extends the earlier [exploration comparison](../2026-09-06-exploration-comparison/README.md). It measures backend work and returned characters/bytes. It does not measure billed tokens, agent reasoning effort, or task completion time.

- Baseline: the earlier study's SHA-256-pinned release binary. Candidate: a fresh release build with the semantic-query and paging repairs.
- Target: the same clean Meridian-Rift commit and 42 recorded source hashes. Both arms read the same checkout. No installation or registration change.
- Cases: spell and vending implementations; airlock, hydroponics and vending references; storage descendants; vending and spell file symbols. These cover five subsystems and all three changed language-listing tools.
- The vending `attackby` implementation query is a negative control: its declaration is inherited, with no implementations inside the requested vending subtree. Empty results are reported explicitly, not counted as recovered implementations.
- Three fresh processes per arm, alternating arm order between rounds. Each process parses once, then runs five repetitions per query. Candidate full/compact order alternates between repetitions. Source files may be in the OS cache; “cold” means a fresh server with no parsed snapshot.
- Follow all continuation pages. Require a terminal untruncated response and the complete `total_count`. Baseline truncation without continuation is an error. Never claim a saving by comparing different first-page sizes.
- Require candidate full and reconstructed compact rows to be exactly equal, in order. Compare complete baseline/candidate row sets separately and record every added/removed row; new variable assignments are an intentional semantic correction, not an output-equivalence win.
- Record each cold-parse latency, parsed process working/private bytes, warm query latency, full response characters/bytes including metadata, page count, binary hashes and clean shutdown. Keep original replies under ignored `target/`; publish only portable aggregates.

Run on Windows after the release build, with no concurrent build or benchmark workload:

```powershell
python docs/audits/2026-09-06-language-queries/run.py --baseline $env:MERIDIAN_BASELINE_BINARY --candidate ./target/release/meridian-mcp.exe --rift-root $env:MERIDIAN_RIFT_ROOT --output ./target/language-query-comparison --rounds 3 --repeats 5 --previous-candidate docs/audits/2026-09-06-language-queries/initial-candidate.json
```

The output directory must not already exist. The runner always closes its owned server, keeps failed raw evidence, rejects a changed baseline binary/target checkout, and leaves the immutable earlier study intact.

The first candidate's complete result record is preserved in `initial-candidate.json`. It exposed a private-memory increase of about 352 MB, motivating a second candidate with shared index text. The second run uses the same protocol and cases; compare its full-row hashes with the first candidate before attributing resource changes to storage sharing.
