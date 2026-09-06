"""Verify and publish portable evidence for the recorded Rift output experiment."""
from pathlib import Path
import collections
import hashlib
import json
import re
import statistics
import subprocess

ROOT = Path.cwd()
OUT = ROOT / "docs/audits/2026-09-06-rift-output"
PROBE = ROOT / "target/rift-output-paired-final"


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def sha(path, lf=False):
    data = path.read_bytes()
    return hashlib.sha256(data.replace(b"\r\n", b"\n") if lf else data).hexdigest()


def write(name, value):
    text = json.dumps(value, indent=2) + "\n"
    assert not re.search(r"(?i)([a-z]:\\|/mnt/[a-z]/|/home/|/Users/|Users\\)", text)
    (OUT / name).write_text(text, encoding="utf-8")


def tests(name, expected):
    path = ROOT / "target" / (name + ".log")
    rows = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;", path.read_text(errors="replace"))
    totals = [sum(int(row[i]) for row in rows) for i in (1, 2, 3)]
    assert totals == expected, (name, totals)
    return dict(zip(("passed", "failed", "ignored"), totals)) | {"log_sha256": sha(path)}


probe = read(PROBE / "summary.json")
assert probe["baseline_sha256"] == "19537b5831aa16abc84f4d89d5c63bd1e673c85a3000444423b45f51ecd03304"
assert probe["baseline_sha256"] == sha(PROBE / "A.exe")
assert probe["candidate_sha256"] == sha(PROBE / "B.exe")
assert len(probe["cases"]) == 66 and len(probe["sessions"]) == 6
raw_hashes = {}
for session in probe["sessions"]:
    assert session["exit_code"] == 0 and session["build"]["complete"]
    assert session["tool_count"] == 36
    counts = collections.Counter(a["status"] for a in session["attempts"])
    assert counts == ({"unverified": 6, "failed": 2} if session["arm"] == "A" else {"unverified": 4, "failed": 9}), counts
    path = PROBE / f"round-{session['round']}-{session['arm']}" / "raw.json"
    raw_hashes[path.relative_to(PROBE).as_posix()] = sha(path)
    responses = {r["id"]: r for r in read(path)["Responses"]}
    for row in probe["cases"]:
        if (row["arm"], row["round"]) != (session["arm"], session["round"]):
            continue
        text = responses[row["request_id"]]["result"]["content"][0]["text"]
        assert len(text.encode()) == row["response_text_utf8_bytes"]
        body = json.loads(text)
        if row["arm"] == "A":
            if row["mode"] in ("many", "flood"):
                assert row["code"] == "limit_exceeded"
            elif row["mode"] in ("error", "malformed", "duplicate", "oversized"):
                assert row["success"]
            elif row["mode"] == "cache":
                assert row["success"] is False and row["code"] == "insufficient_evidence"
            continue
        assert row["response_text_utf8_bytes"] <= 512 * 1024
        assert row["success"] == (row["mode"] in ("quiet", "flood", "cache"))
        assert row["artifact_dmb_sha256"] == row["actual_dmb_sha256"]
        assert row["artifact_rsc_sha256"] == row["actual_rsc_sha256"]
        assert row["provenance_status"] == "unverified" and not row["has_build_record_id"]
        summary = row["diagnostic_summary"]
        assert summary["output_complete"] and summary["analysis_complete"] == (row["mode"] != "oversized")
        total = {"many": 30000, "error": 1}.get(row["mode"], 0)
        assert summary["errors"] == total
        limit = int(row["variant"].split("-")[1]) if row["variant"].startswith("limit-") else 50
        assert len(body["diagnostics"]) == min(limit, total)
        assert summary["errors_detail"]["omitted"] == total - len(body["diagnostics"])
        if row["variant"] != "default":
            assert "stdout" not in body and "stderr" not in body
        if row["mode"] == "many" and limit:
            assert row["first_diagnostic"] == "fixture.dm:1: error: failure 1"
        if row["mode"] == "cache":
            assert row["evidence"] == "valid_cache_hit" and row["cache_evidence"] == "Skipping 'dm' (up to date)"
assert len({r["actual_dmb_sha256"] for r in probe["cases"] if r["mode"] != "missing"}) == 1
assert len({r["actual_rsc_sha256"] for r in probe["cases"] if r["mode"] != "missing"}) == 1

metrics = []
for mode, variant in dict.fromkeys((r["mode"], r["variant"]) for r in probe["cases"]):
    result = {"mode": mode, "variant": variant}
    for arm, name in (("A", "baseline"), ("B", "candidate")):
        rows = [r for r in probe["cases"] if (r["mode"], r["variant"], r["arm"]) == (mode, variant, arm)]
        if not rows:
            continue
        result[name] = {field: statistics.median(r[field] for r in rows) if all(r[field] is not None for r in rows) else None for field in ("response_text_utf8_bytes", "wire_bytes", "roundtrip_ms", "build_duration_ms")}
        result[name]["success"] = rows[0]["success"]
        result[name]["code"] = rows[0]["code"]
    metrics.append(result)

native = read(ROOT / "target/rift-output-native/summary.json")
assert native["binary_sha256"] == probe["candidate_sha256"]
assert native["natural_mcp_exit_code"] == 0 and native["byond_version"] == "5.0.516.1687"
assert collections.Counter(a["status"] for a in native["attempts"]) == {"unverified": 1, "failed": 1}
for row in native["cases"]:
    assert row["success"] == (row["case"] == "success")
    assert row["provenance_status"] == "unverified"
    assert row["diagnostic_summary"]["analysis_complete"]
    assert row["diagnostic_summary"]["errors"] == (0 if row["success"] else 1)
    assert row["stdout_capture_truncated_bytes"] > 0
    if row["success"]:
        assert row["dmb_sha256"] and row["rsc_sha256"]
    else:
        assert row["code"] == "build_failed" and re.search(r"fixture\.dm:5:\s*error:", row["diagnostics"][0])

gates = {}
for platform, count, ignored in (("windows", 466, 5), ("linux", 454, 6)):
    gates[platform] = tests(f"rift-output-{platform}-final-tests", [count, 0, ignored])
    path = ROOT / f"target/rift-output-{platform}-clippy.log"
    log = path.read_text(errors="replace")
    assert "Finished `dev` profile" in log and not re.search(r"^error", log, re.M)
    gates[platform] |= {"strict_clippy": "passed", "clippy_log_sha256": sha(path)}

sources = ["src/parameters.rs", "src/process.rs", "src/process/output.rs", "src/tools/mod.rs", "src/tools/compile.rs", "src/tools/compile/diagnostics.rs", "src/tools/build_response.rs", "src/tools/rift.rs", "src/tools/rift/output.rs", "tests/mcp_conformance.rs", "tests/rift_output.rs", "tests/fixtures/rift_output.rs"]
sources += [f"docs/audits/2026-09-06-rift-output/{name}" for name in ("probe.ps1", "native-probe.ps1", "record.py")]
record = {
    "schema": 1, "base_revision": "59d11ac", "rust_version": subprocess.check_output(["rustc", "+1.95.0", "--version"], text=True).strip(),
    "source_sha256_lf": {p: sha(ROOT / p, True) for p in sources}, "gates": gates,
    "red": tests("rift-output-red", [1, 4, 0]),
    "focused": tests("rift-output-focused-valid", [22, 0, 0]),
    "initial_windows": tests("rift-output-windows-tests", [465, 1, 5]),
    "initial_windows_explanation": "The exact schema-field conformance test still expected the old option set. Updated its explicit allowlist and added output-bound assertions; the full rerun passed.",
    "probe_harness_correction": "The first baseline-only session completed normally; the recorder then indexed a missing diagnostics array in a transport size-error reply. Kept that raw run and corrected null handling before starting six fresh sessions.",
    "comparison": {"file": "comparison.json", "cases": 66, "matched_default_calls": 54, "candidate_output_control_calls": 12, "sessions": 6,
        "baseline_false_successes": 12, "baseline_lost_cache_hits": 3, "baseline_transport_size_errors": 6, "baseline_missing_failed_attempts": 3,
        "identical_synthetic_artifact_pairs": 60, "raw_sha256": raw_hashes},
    "native": {"file": "native.json", "cases": 2, "raw_sha256": sha(ROOT / "target/rift-output-native/raw.json")},
    "boundaries": [
        "Local Linux under WSL 1 does not replace hosted native Ubuntu CI; Rift launch is Windows-only and Linux exercises the portable collector/formatter and unsupported-platform gate.",
        "The previously observed WSL 1 guardian readiness timeout remains unexplained; a passing run does not establish a repair.",
        "Synthetic comparisons use debug MCP binaries and an owned fake compiler, with equal artifacts and deliberately changed failure classification.",
        "The native gate uses a small owned RIFT_BUILD.cmd wrapper and actual DreamMaker, not the Meridian-Rift production build or playtest.",
        "Successful Rift wrappers still do not prove the compiler input closure or establish verified provenance.",
        "Three alternating rounds measure response bytes and record latency; they do not establish a speedup, memory saving, model-token saving or reduced reasoning cost.",
        "Output analysis occurs during pipe reads, changing what duration_ms includes; prefer end-to-end latency when inspecting timings.",
        "Hosted CI, installation, documentation cleanup, remaining tool inventory and release handoff remain separate work."
    ],
}
write("comparison.json", probe)
write("metrics.json", metrics)
write("native.json", native)
write("verification.json", record)
print(json.dumps({"gates": gates, "cases": 66, "repaired_false_successes": 12}, indent=2))
