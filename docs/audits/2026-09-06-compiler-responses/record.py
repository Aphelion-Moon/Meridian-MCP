"""Validate repository-local probe/log evidence and write portable audit records."""
from pathlib import Path
import hashlib
import json
import re
import statistics
import subprocess

ROOT = Path.cwd()
OUT = ROOT / "docs/audits/2026-09-06-compiler-responses"
PROBE = ROOT / "target/compiler-responses-paired"


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def sha(path, lf=False):
    data = path.read_bytes()
    return hashlib.sha256(data.replace(b"\r\n", b"\n") if lf else data).hexdigest()


def write(name, value):
    text = json.dumps(value, indent=2) + "\n"
    assert not re.search(r"(?i)([a-z]:\\|/mnt/[a-z]/|/home/|/Users/|Users\\)", text)
    (OUT / name).write_text(text, encoding="utf-8")


def test_result(name, expected):
    path = ROOT / "target" / (name + ".log")
    rows = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;", path.read_text(errors="replace"))
    totals = [sum(int(row[i]) for row in rows) for i in (1, 2, 3)]
    assert totals == expected, (name, totals)
    return dict(zip(("passed", "failed", "ignored"), totals)) | {"log_sha256": sha(path)}


probe = read(PROBE / "summary.json")
assert probe["baseline_sha256"] == "a706227b7058f3c719ddb287a109b3c313d5500cf8bb005020a23d6837c0f224"
assert probe["baseline_sha256"] == sha(PROBE / "A.exe")
assert probe["candidate_sha256"] == sha(PROBE / "B.exe")
assert len(probe["cases"]) == 72 and len(probe["sessions"]) == 6
assert all(session["exit_code"] == 0 and session["build"]["complete"] for session in probe["sessions"])

raw_bodies = {}
raw_hashes = {}
for session in probe["sessions"]:
    key = (session["round"], session["arm"])
    path = PROBE / f"round-{key[0]}-{key[1]}" / "raw.json"
    raw = read(path)
    raw_hashes[path.relative_to(PROBE).as_posix()] = sha(path)
    responses = {r["id"]: r for r in raw["Responses"]}
    rows = [r for r in probe["cases"] if (r["round"], r["arm"]) == key]
    for request_id, row in enumerate(rows, 10):
        text = responses[request_id]["result"]["content"][0]["text"]
        body = json.loads(text)
        assert len(text.encode()) == row["response_text_utf8_bytes"]
        row["overflow_output_bytes"] = body.get("details", {}).get("output_bytes")
        raw_bodies[key + (row["mode"], row["variant"])] = body

equivalent_pairs = 0
for key, candidate in raw_bodies.items():
    round_number, arm, mode, variant = key
    if arm != "B":
        continue
    baseline = raw_bodies[(round_number, "A", mode, "default")]
    assert "success" in candidate
    if "success" not in baseline:
        continue
    for field in ("success", "compiler_succeeded", "exit_code", "termination", "dmb_exists", "dmb_updated", "provenance_status"):
        assert candidate[field] == baseline[field], (key, field)
    for field in ("sha256", "size", "exists"):
        assert candidate["artifact_after"][field] == baseline["artifact_after"][field]
    for severity in ("errors", "warnings"):
        assert candidate["diagnostic_summary"][severity] == len(baseline[severity])
        for i, row in enumerate(candidate[severity]):
            original = baseline[severity][i]
            compared = dict(row)
            if compared.pop("message_truncated", False):
                assert compared.pop("message_utf8_bytes") == len(original["message"].encode())
                assert original["message"].startswith(compared["message"])
                compared["message"] = original["message"]
            assert compared == original
    for stream in ("stdout", "stderr"):
        summary = candidate["output_summary"][stream]
        assert summary["available_utf8_bytes"] == len(baseline[stream].encode())
        if variant == "default":
            assert baseline[stream].endswith(candidate[stream])
        else:
            assert stream not in candidate
    equivalent_pairs += 1
assert equivalent_pairs == 30
assert len({r["actual_artifact_sha256"] for r in probe["cases"]}) == 1

metrics = []
for mode in dict.fromkeys(r["mode"] for r in probe["cases"]):
    row = {"mode": mode}
    for arm, variant, name in (("A", "default", "baseline"), ("B", "default", "default"), ("B", "no_output", "no_output")):
        cases = [r for r in probe["cases"] if (r["mode"], r["arm"], r["variant"]) == (mode, arm, variant)]
        row[name] = {key: statistics.median(r[key] for r in cases) for key in ("response_text_utf8_bytes", "wire_bytes", "roundtrip_ms", "request_bytes")}
        row[name]["code"] = cases[0]["code"]
    metrics.append(row)

native = read(ROOT / "target/compiler-responses-native/summary.json")
assert native["binary_sha256"] == probe["candidate_sha256"] and native["exit_code"] == 0
assert native["byond_version"] == "5.0.516.1687"
for case in native["cases"][:3]:
    assert not case["parse_error"] and not case["is_error"] and case["success"]
    assert case["dmb_exists"] and case["dmb_updated"] and case["provenance_status"] == "verified"
assert native["cases"][3]["code"] == "invalid_input" and not native["cases"][3]["dmb_exists"]

gates = {}
for platform, expected in (("windows", [450, 0, 5]), ("linux", [443, 0, 6])):
    gates[platform] = test_result(f"compiler-responses-{platform}-tests", expected)
    log = ROOT / f"target/compiler-responses-{platform}-clippy.log"
    text = log.read_text(errors="replace")
    assert "Finished `dev` profile" in text and not re.search(r"^error", text, re.M)
    gates[platform] |= {"strict_clippy": "passed", "clippy_log_sha256": sha(log)}
for name, marker in (("capability-audit", "128 source capabilities"), ("capability-drift", "143 assertions"), ("dmdoc", "documentation stdio fixture passed")):
    path = ROOT / f"target/compiler-responses-{name}.log"
    assert marker in path.read_text(errors="replace")
    gates[name] = {"status": "passed", "log_sha256": sha(path)}

sources = ["src/tools/compile.rs", "src/tools/compile/arguments.rs", "src/tools/compile/response.rs", "src/tools/mod.rs", "tests/compiler_responses.rs", "tests/fixtures/compiler_output.rs", "docs/audits/2026-09-06-compiler-responses/probe.ps1", "docs/audits/2026-09-06-compiler-responses/record.py"]
record = {
    "schema": 1, "base_revision": "485f716", "rust_version": subprocess.check_output(["rustc", "+1.95.0", "--version"], text=True).strip(),
    "source_sha256_lf": {p: sha(ROOT / p, True) for p in sources}, "gates": gates,
    "before": test_result("compiler-responses-red", [1, 3, 0]),
    "initial_focused": test_result("compiler-responses-focused", [20, 0, 0]),
    "comparison": {"file": "comparison.json", "cases": 72, "sessions": 6, "equivalent_pairs_with_baseline_status": equivalent_pairs,
                   "candidate_statuses_retained": 48, "baseline_overflow_cases": 9, "raw_sha256": raw_hashes},
    "native": {"file": "native.json", "verified_fresh_dmb_cases": 3, "invalid_request_created_dmb": False, "natural_mcp_exit_code": 0},
    "boundaries": [
        "Local Windows and Linux under WSL 1, not hosted native Ubuntu CI.",
        "Output comparison uses an owned synthetic compiler and debug MCP binaries.",
        "Native compiler evidence is a small owned Windows BYOND fixture, not a full Meridian-Rift build.",
        "Latency includes client JSON parsing and concurrent qualification activity; no speedup or memory claim.",
        "Actual model tokens and thinking cost were not measured; response and wire bytes are recorded.",
        "Capture loss limits diagnostic completeness; omitted text has no later retrieval API.",
        "The earlier intermittent WSL ownership-fixture failure remains unexplained.",
        "Hosted CI, live debugger/Tracy qualification, installed-MCP replacement and broader artifact review remain separate."
    ],
}
write("comparison.json", probe)
write("metrics.json", metrics)
write("native.json", native)
write("verification.json", record)
print(json.dumps({"gates": gates, "equivalent_pairs": equivalent_pairs, "cases": 72}, indent=2))
