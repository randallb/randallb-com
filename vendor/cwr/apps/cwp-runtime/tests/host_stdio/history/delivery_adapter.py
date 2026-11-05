#!/usr/bin/env python3
"""Visible growing history, acknowledged by the real adapter protocol."""
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


def emit(message):
    print(json.dumps({"invocation_ref": reference, **message}), flush=True)


def wait_for(name):
    deadline = time.monotonic() + 40
    while not (root / name).exists():
        assert time.monotonic() < deadline, name
        time.sleep(0.005)


def updates(start, end):
    for index in range(start, end):
        emit({"type": "progress", "sequence": index + 1, "status": 102, "published": True,
              "summary": f"visible update {index:03}: " + "résumé 🐢 " * 32})


def acknowledge(sequence, count):
    emit({"type": "activity", "sequence": sequence,
          "event": {"kind": "provider_event", "provider": "codex",
                    "method": "fixture/delivery-barrier", "data": {"updates": count}}})
    assert json.loads(sys.stdin.readline()) == {
        "type": "activity_result", "invocation_ref": reference, "sequence": sequence,
    }


(root / "ready").touch()
wait_for("warmup")
updates(0, 290)
acknowledge(1, 290)
(root / "warmup-done").touch()
# The test drains its first host and orders a barrier before allowing the final
# burst. Slow bulk fixture construction must not consume the socket stall budget.
wait_for("burst")
updates(290, 300)
acknowledge(2, 300)
(root / "burst-done").touch()
wait_for("finish")
emit({"type": "result", "sequence": 301, "status": 200,
      "summary": "Delivery finished", "result": {"text": "Delivery finished"}})
