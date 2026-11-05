#!/usr/bin/env python3
"""Synthetic tool/progress events held until the subscriber observes each phase."""
import json
from pathlib import Path
import sys
import time

invocation = json.loads(sys.stdin.readline())["invocation"]
if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass
reference = invocation["invocation_ref"]
root = Path(__file__).parent


def activity(sequence, event, marker):
    print(json.dumps({"type": "activity", "invocation_ref": reference,
                      "sequence": sequence, "event": event}), flush=True)
    response = json.loads(sys.stdin.readline())
    assert response["type"] == "activity_result" and not response.get("error"), response
    wait(marker)


def wait(marker):
    until = time.monotonic() + 10
    while not root.joinpath(marker).exists():
        assert time.monotonic() < until, marker
        time.sleep(0.01)


activity(1, {"kind": "execution_activity", "activity": {
    "phase": "using_tool", "state": "started"}}, "observed-tool")
print(json.dumps({"type": "progress", "invocation_ref": reference,
                  "sequence": 1, "status": 102, "published": True,
                  "summary": "Fixture progress"}), flush=True)
wait("observed-progress")
print(json.dumps({"type": "result", "invocation_ref": reference,
                  "sequence": 2, "status": 200, "summary": "Fixture complete",
                  "result": {"text": "Fixture complete"}}), flush=True)
