#!/usr/bin/env python3
"""Create an independent root, holding its creator until status is delivered."""
import json
from pathlib import Path
import sys
import time

invocation = json.loads(sys.stdin.readline())["invocation"]
if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass
reference = invocation["invocation_ref"]


def host_call(call_id, operation, payload):
    print(json.dumps({"type": "call", "invocation_ref": reference,
                      "call_id": call_id, "operation": operation, "input": payload}), flush=True)
    response = json.loads(sys.stdin.readline())
    if call_id == "create-root":
        Path(__file__).with_name("create-response.json").write_text(json.dumps(response))
    assert response["type"] == "call_result" and not response.get("error"), response
    return response["result"]


def tool_activity(sequence, phase, marker):
    print(json.dumps({"type": "activity", "invocation_ref": reference,
                      "sequence": sequence, "event": {"kind": "execution_activity",
                      "activity": {"phase": phase, "state": "started"}}}), flush=True)
    acknowledgment = json.loads(sys.stdin.readline())
    assert acknowledgment["type"] == "activity_result" and not acknowledgment.get("error"), acknowledgment
    until = time.monotonic() + 10
    while not Path(__file__).with_name(marker).exists():
        assert time.monotonic() < until, marker
        time.sleep(0.01)


mode = invocation["input"].get("mode", "own")
if mode != "child":
    created = host_call("create-root", "cwr.create_conversation", {
        "owner_context_ref": "workspace", "text": "@helper pending created work"})
    assert created["invocation_ref"], created

if mode == "delegate":
    # No parent activity is emitted between the independent admission and the
    # synchronous child's first callback. Its ancestry must carry the effect.
    discovered = host_call("discover-child", "cwr.list_actions", {})
    target = next(action["target"] for action in discovered["actions"]
                  if action["activation"] == "@child--run--task" and action["available"])
    response = host_call("invoke-child", "cwr.invoke_action", {
        "target": target, "input": {"mode": "child"}})
    assert response["status"] == 200, response
    Path(__file__).with_name("parent-resumed").write_text("invoke-child returned")
else:
    # The next ordinary activity callback is the existing publication boundary
    # after a host call; no new callback or event shape is introduced for it.
    tool_activity(1, "using_tool", "observed-created")
    tool_activity(2, "running_command", "observed-followup")
print(json.dumps({"type": "result", "invocation_ref": reference,
                  "sequence": 1, "status": 200, "summary": "Created independent work",
                  "result": {"text": "Created independent work"}}), flush=True)
