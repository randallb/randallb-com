#!/usr/bin/env python3
"""Fake Codex process protocol: 33 history parts, with synchronous checkpoints."""
import hashlib
import json
import pathlib
import sys
import time

root = pathlib.Path(__file__).parent
invocation = json.loads(sys.stdin.readline())["invocation"]
assert pathlib.Path(invocation["integration_params"]["codex_executable"]).samefile(root / "codex")
if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass
reference = invocation["invocation_ref"]
activity_sequence = 0
checkpoints = []


def emit(message):
    print(json.dumps({"invocation_ref": reference, **message}), flush=True)


def activity(event):
    global activity_sequence
    activity_sequence += 1
    emit({"type": "activity", "sequence": activity_sequence, "event": event})
    # Moving on before this response would no longer exercise synchronous retention.
    assert json.loads(sys.stdin.readline()) == {
        "type": "activity_result", "invocation_ref": reference,
        "sequence": activity_sequence,
    }


def wait_for(name):
    deadline = time.monotonic() + 20
    while not (root / name).exists():
        assert time.monotonic() < deadline, name
        time.sleep(0.005)


checkpoint = {
    "version": 2, "invocation_ref": reference,
    "integration_ref": invocation["integration_ref"],
    "source_digest": hashlib.sha256(json.dumps(invocation["thread"]).encode()).hexdigest(),
    "initialization_digest": "fake-codex-history-v1",
    "context_snapshot": invocation["thread"].get("context_snapshot"),
    "thread_id": "history-test", "confirmed_items": 0,
    "pending": False, "live_started": False,
}


def persist():
    activity({"kind": "provider_event", "provider": "codex",
              "method": "cwr.adapter_checkpoint", "data": checkpoint})
    checkpoints.append(checkpoint.copy())


activity({"kind": "provider_thread", "provider": "codex", "thread_id": "history-test"})
(root / "ready").touch()
wait_for("restore")
for part in range(33):
    checkpoint["pending"] = True
    persist()
    # Simulated provider acknowledgement of this history part.
    checkpoint["confirmed_items"] = part + 1
    checkpoint["pending"] = False
    persist()
checkpoint["live_started"] = True
persist()
(root / "acknowledged.json").write_text(json.dumps(checkpoints))
wait_for("visible")
emit({"type": "progress", "sequence": 1, "status": 102,
      "summary": "Conversation history restored: 33/33 parts."})
activity({"kind": "provider_event", "provider": "codex", "method": "item/completed",
          "data": {"item": {"id": "history-tool", "type": "commandExecution",
                            "command": "echo history-restored", "status": "completed",
                            "exitCode": 0, "aggregatedOutput": "history-restored"}}})
(root / "visible-ready").touch()
wait_for("finish")
emit({"type": "result", "sequence": 2, "status": 200,
      "summary": "Restoration finished", "result": {"text": "Restoration finished"}})
