"""Bind the recorded streaming-diagnostic experiment to its source and logs."""
from pathlib import Path
import collections
import hashlib
import json
import re
import statistics
import subprocess

ROOT = Path.cwd()
OUT = ROOT / "docs/audits/2026-09-06-compiler-diagnostics"
PROBE = ROOT / "target/compiler-diagnostics-paired"


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
assert probe["baseline_sha256"] == "9f46c7c64620e2efedc0ec1d490a05e8094b4ba0a63191de8493b1bbe6e6abd9"
assert probe["baseline_sha256"] == sha(PROBE / "A.exe")
assert probe["candidate_sha256"] == sha(PROBE / "B.exe")
assert len(probe["cases"]) == 72 and len(probe["sessions"]) == 6
raw_hashes = {}
for session in probe["sessions"]:
    assert session["exit_code"] == 0 and session["build"]["complete"]
    counts = collections.Counter(attempt["status"] for attempt in session["attempts"])
    assert counts == ({"succeeded": 9, "failed": 3} if session["arm"] == "A" else {"succeeded": 3, "failed": 9})
    if session["arm"] == "B":
        codes = collections.Counter(a.get("code") for a in session["attempts"] if a["status"] == "failed")
        assert codes == {"compiler_failed": 6, "diagnostic_analysis_incomplete": 3}
    path = PROBE / f"round-{session['round']}-{session['arm']}" / "raw.json"
    raw_hashes[path.relative_to(PROBE).as_posix()] = sha(path)
    responses = {r["id"]: r for r in read(path)["Responses"]}
    for row in probe["cases"]:
        if (row["arm"], row["round"]) != (session["arm"], session["round"]):
            continue
        text = responses[row["request_id"]]["result"]["content"][0]["text"]
        assert len(text.encode()) == row["response_text_utf8_bytes"]
        assert row["artifact_sha256"] == row["actual_artifact_sha256"]
        if row["arm"] == "A" and row["mode"] == "early_error":
            assert row["success"] and row["provenance_status"] == "verified"
            assert row["diagnostic_summary"]["errors"] == 0
        if row["arm"] == "B":
            body = json.loads(text)
            expected = {"quiet": (0, 0), "early_error": (1, 1), "many_diagnostics": (30000, 20000), "overlong_line": (0, 0)}[row["mode"]]
            assert row["success"] == (row["mode"] == "quiet")
            assert row["response_text_utf8_bytes"] <= 512 * 1024
            assert row["diagnostic_summary"]["analysis_complete"] == (row["mode"] != "overlong_line")
            assert row["diagnostic_summary"]["output_complete"]
            for name, total in zip(("errors", "warnings"), expected):
                assert row["diagnostic_summary"][name] == total
                assert len(body[name]) == min(row["limit"], total)
                assert row["diagnostic_summary"][name + "_detail"]["omitted"] == total - len(body[name])
            if row["mode"] == "many_diagnostics" and row["limit"]:
                assert row["first_error_line"] == 1
assert len({r["actual_artifact_sha256"] for r in probe["cases"]}) == 1

metrics = []
for mode in ("quiet", "early_error", "many_diagnostics", "overlong_line"):
    for limit in (0, 2, 200):
        result = {"mode": mode, "diagnostic_limit": limit}
        for arm, name in (("A", "baseline"), ("B", "candidate")):
            rows = [r for r in probe["cases"] if (r["mode"], r["limit"], r["arm"]) == (mode, limit, arm)]
            result[name] = {field: statistics.median(r[field] for r in rows) for field in ("response_text_utf8_bytes", "wire_bytes", "roundtrip_ms", "compile_duration_ms")}
        metrics.append(result)

native = read(ROOT / "target/compiler-diagnostics-native/summary.json")
assert native["binary_sha256"] == probe["candidate_sha256"] and native["exit_code"] == 0
assert native["byond_version"] == "5.0.516.1687"
for case in native["cases"][:3]:
    assert not case["parse_error"] and not case["is_error"] and case["success"]
    assert case["dmb_exists"] and case["dmb_updated"] and case["provenance_status"] == "verified"
assert native["cases"][3]["code"] == "invalid_input" and not native["cases"][3]["dmb_exists"]
native_error = read(ROOT / "target/compiler-diagnostics-native-error/summary.json")
assert native_error["binary_sha256"] == probe["candidate_sha256"]
assert native_error["natural_mcp_exit_code"] == 0 and not native_error["success"] and not native_error["dmb_exists"]
assert native_error["diagnostic_summary"]["analysis_complete"] and native_error["diagnostic_summary"]["errors"] == 1
assert native_error["diagnostics"][0]["line"] == 2

gates = {}
for platform, expected in (("windows", [458, 0, 5]), ("linux", [451, 0, 6])):
    gates[platform] = tests(f"compiler-diagnostics-{platform}-final-tests", expected)
    path = ROOT / f"target/compiler-diagnostics-{platform}-final-clippy.log"
    text = path.read_text(errors="replace")
    assert "Finished `dev` profile" in text and not re.search(r"^error", text, re.M)
    gates[platform] |= {"strict_clippy": "passed", "clippy_log_sha256": sha(path)}
path = ROOT / "target/compiler-diagnostics-dmdoc.log"
assert "documentation stdio fixture passed" in path.read_text(errors="replace")
gates["dmdoc"] = {"status": "passed", "log_sha256": sha(path)}

sources = ["src/process.rs", "src/tools/compile.rs", "src/tools/compile/diagnostics.rs", "src/tools/compile/response.rs", "tests/compiler_responses.rs", "tests/fixtures/compiler_output.rs", "tests/process_runner.rs", "docs/audits/2026-09-06-compiler-diagnostics/probe.ps1", "docs/audits/2026-09-06-compiler-diagnostics/native-error-probe.ps1", "docs/audits/2026-09-06-compiler-diagnostics/record.py"]
record = {
    "schema": 1, "base_revision": "ddebb0c", "rust_version": subprocess.check_output(["rustc", "+1.95.0", "--version"], text=True).strip(),
    "source_sha256_lf": {p: sha(ROOT / p, True) for p in sources}, "gates": gates,
    "red": tests("compiler-diagnostics-red", [5, 3, 0]),
    "initial_focused": tests("compiler-diagnostics-focused", [32, 0, 1]),
    "strengthened_provenance_fixture": tests("compiler-diagnostics-provenance", [1, 0, 0]),
    "initial_full": {
        "windows": tests("compiler-diagnostics-windows-tests", [457, 1, 5]),
        "linux": tests("compiler-diagnostics-linux-tests", [449, 2, 6]),
        "explanation": "The new test incorrectly expected stale provenance without a prior verified build; its fixture now establishes that build and verifies failure invalidates it. Linux also reproduced the earlier guardian readiness timeout. The complete Linux rerun ran without concurrent Windows builds; the timeout cause remains unproved."
    },
    "comparison": {"file": "comparison.json", "cases": 72, "sessions": 6, "baseline_early_error_false_successes": 9, "candidate_early_error_failures": 9, "raw_sha256": raw_hashes},
    "native": {"positive": "native.json", "compiler_error": "native-error.json", "verified_fresh_dmb_cases": 3, "native_error_count": 1, "native_error_line": 2, "natural_mcp_exit_code": 0},
    "boundaries": [
        "Local Linux/WSL 1 qualification does not replace hosted native Ubuntu CI.",
        "The earlier guardian readiness timeout recurred in the initial parallel qualification and remains unexplained.",
        "Synthetic output comparisons use debug MCP binaries and an owned fake compiler; artifact hashes match, but outcome and counts intentionally change.",
        "Most streaming analysis now occurs inside the process runner, so duration_ms covers different work than the baseline; use end-to-end latency for comparisons.",
        "Three alternating rounds and concurrent Windows qualification do not establish a compiler speedup, memory saving or model-token saving.",
        "A line larger than 1 MiB or incomplete pipe reads leaves analysis incomplete and prevents verified build success.",
        "Rift build tail classification, the remaining 62-tool inventory, live helper gates and release handoff remain active; the installed MCP is unchanged."
    ],
}
write("comparison.json", probe)
write("metrics.json", metrics)
write("native.json", native)
write("native-error.json", native_error)
write("verification.json", record)
print(json.dumps({"gates": gates, "cases": 72, "repaired_false_successes": 9}, indent=2))
