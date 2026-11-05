#!/usr/bin/env python3
"""Deterministic process adapter: finish only when the test releases its work."""
import json
import pathlib
import sys
import time

invocation = json.loads(sys.stdin.readline())["invocation"]
if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass
directory = pathlib.Path(__file__).parent
with (directory / "started").open("a") as marker:
    marker.write(invocation["invocation_ref"] + "\n")
while not (directory / "release").exists():
    time.sleep(0.01)
print(json.dumps({
    "type": "result",
    "invocation_ref": invocation["invocation_ref"],
    "sequence": 1,
    "status": 200,
    "summary": "Finished before restart",
    "result": {"text": "Finished before restart"},
}), flush=True)
