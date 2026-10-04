# Native evidence analysis

`dm_native_evidence_summary` and `dm_native_evidence_compare` inspect technical evidence without changing or publishing its raw files. Supported kinds are explicit: `byond_proc_profile_json`, `byond_sendmaps_json`, `performance_csv`, `runtime_jsonl`, and `event_jsonl`. There is no format autodetection or executable parser extension.

Each run accepts at most 32 regular evidence files, 256 MiB per file and 1 GiB of raw bytes combined. Reads stop at the smaller of the file ceiling and the remaining run budget, with one extra byte to detect overflow. Accounting and hashes use the bytes actually read, including when metadata understates a file's size. The native-evidence and memory summary/comparison tools share the server's bounded blocking-job pool for reading, analysis and serialization.

BYOND proc and sendmaps documents are a cumulative snapshot. Performance CSV is an interval series. Runtime and mapped event JSONL are event streams. Wall-clock time, BYOND world deciseconds, and artifact-local sample indexes remain separate. Named ranges are half-open: a timestamp at the end belongs to the following phase or remains unassigned. Cumulative evidence before `game_start` is labeled `pre_game_cumulative`; it is never stretched across later phases.

CSV preserves signed integer wall-clock milliseconds and world deciseconds before phase assignment. Wall-clock fields also accept RFC3339 timestamps.

Numeric summaries use deterministic type-7 percentiles, including p50, p95, and p99, plus count, missing count, minimum, maximum, compensated mean, and sample standard deviation. Missing cells do not become zero. Unknown or unavailable measures remain explicit. Repeated comparisons with at least three runs also report coefficient of variation when the mean is nonzero.

## Privacy and interpretation

The default redaction is mandatory for player, client, account, key/ckey, mob, and Discord identifiers and their ID aliases. Protected fields cannot become group keys. Matching `name=value` segments in returned text are replaced with `<redacted>`. Raw artifacts remain local and unchanged.

Protected fields are removed before numeric metrics, clocks or groups are interpreted, even when explicitly selected. Field matching is case-insensitive and accepts underscore or hyphen ID aliases; returned text uses the same aliases.

An unverified summary is useful for local inspection, but comparison requires every run to carry the same current verified managed build, DMB, workload, phase, artifact-kind sequence, and metric coverage. A mismatch returns `evidence_identity_mismatch` before metric calculation. A fixture-green result is bounded technical evidence, not a production performance conclusion.

## Summary example

```json
{
  "artifacts": [{
    "kind": "performance_csv",
    "path": "evidence/performance.csv",
    "options": {"selected_metrics": ["tick_usage"], "wall_time_field": "timestamp"}
  }],
  "phases": [{
    "id": "steady",
    "wall_start": "2026-01-01T00:00:00Z",
    "wall_end": "2026-01-01T00:05:00Z"
  }]
}
```

The schema-1 response contains each artifact's root-relative path, byte count, SHA-256, fixed semantics, classification, assigned phase when unambiguous, numeric summaries, redaction counts, and identity-verification status.

## Comparison example

```json
{
  "runs": [
    {"artifacts": [{"kind": "performance_csv", "path": "evidence/run-a.csv"}], "dmb_path": "build/game.dmb"},
    {"artifacts": [{"kind": "performance_csv", "path": "evidence/run-b.csv"}], "dmb_path": "build/game.dmb"}
  ]
}
```

The comparison response names the verified build record, run count, canonical metric keys, input values, two-run absolute/percentage deltas, and distributions for repeated runs.
