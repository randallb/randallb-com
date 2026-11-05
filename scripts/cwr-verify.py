#!/usr/bin/env python3
"""Exercise the real runtime: passing checks, failing test, restored checks."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
fixture = root / "worker_test.ts"
original = fixture.read_bytes()
needle = b".status !== 405"
assert needle in original, "failure fixture no longer matches test"
results = []

def invoke(label):
    run = subprocess.run(["sh", "scripts/cwr-check.sh"], cwd=root, text=True, capture_output=True)
    results.append({"label": label, "caller_exit": run.returncode, "stdout": run.stdout, "stderr": run.stderr})
    response = json.loads(run.stdout)
    results[-1]["runtime_status"] = response["status"]
    return run.returncode

try:
    assert invoke("passing") == 0 and results[-1]["runtime_status"] == 200, results[-1]
    fixture.write_bytes(original.replace(needle, b".status !== 499", 1))
    assert invoke("deliberate-test-failure") != 0, results[-1]
    assert results[-1]["runtime_status"] == 500, results[-1]
    assert "command_failed" in results[-1]["stdout"], results[-1]
    assert "Error: POST" in results[-1]["stdout"], results[-1]
finally:
    fixture.write_bytes(original)
assert invoke("restored") == 0 and results[-1]["runtime_status"] == 200, results[-1]
assert fixture.read_bytes() == original
print(json.dumps(results, indent=2))
