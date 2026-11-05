#!/usr/bin/env python3
"""Barrier-controlled synthetic publication workload for the real process adapter."""
import json
import os
from pathlib import Path
import sys
import time

if sys.argv[1:] == ["--configure"]:
    data = Path(os.environ["CWR_ADAPTER_DATA_DIRECTORY"])
    home = data / "provider-home"
    home.mkdir(mode=0o700, exist_ok=True)
    print(json.dumps({"data_directory": str(data), "provider_home": str(home)}))
    sys.exit(0)

root = Path(__file__).parent
config = json.loads((root / "config.json").read_text())
invocation = json.loads(sys.stdin.readline())["invocation"]
worker = invocation["input"]["worker"]
assert isinstance(worker, int) and 0 <= worker < config["workers"]
directory = root / f"worker-{worker}"
directory.mkdir()
reference = invocation["invocation_ref"]
identity = directory / "pid.json"
temporary = identity.with_suffix(".tmp")
temporary.write_text(json.dumps({"pid": os.getpid(), "pgid": os.getpgrp(),
                                 "invocation_ref": reference}))
temporary.replace(identity)

if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass

activity_sequence = 0


def emit(message):
    print(json.dumps({"invocation_ref": reference, **message}), flush=True)


def tool(item_id, label, completed):
    global activity_sequence
    activity_sequence += 1
    emit({"type": "activity", "sequence": activity_sequence,
          "event": {"kind": "provider_event", "provider": "codex",
                    "method": "item/completed" if completed else "item/started",
                    "data": {"item": {"id": item_id, "type": "commandExecution",
                                      "status": "completed" if completed else "inProgress",
                                      "progress_summary": label, "command": "echo fixture",
                                      "aggregatedOutput": "fixture" if completed else "",
                                      "exitCode": 0}}}})
    response = json.loads(sys.stdin.readline())
    assert response == {"type": "activity_result", "invocation_ref": reference,
                        "sequence": activity_sequence}, response


def wait_for(phase):
    deadline = time.monotonic() + 90
    path = root / f"release-{phase}"
    while not path.exists():
        assert time.monotonic() < deadline, f"timed out waiting for {path.name}"
        time.sleep(0.002)


def output(event):
    emit({"type": "model_output",
          "output": {"model_run_ref": invocation["model_run_ref"], "provider": "fixture",
                     "response_or_turn_id": "replay", "phase": "final_answer",
                     "event": event}})


for index in range(config["relevant_history"]):
    tool(f"seed-{index}", f"replay-w{worker}-seed-{index}", True)
(directory / "ready").touch()

with (directory / "emissions.jsonl").open("w") as emissions:
    for phase, kind in enumerate(("progress", "tool_start", "stream", "tool_complete", "terminal")):
        wait_for(phase)
        emissions.write(json.dumps({"kind": kind, "phase": phase,
                                    "time_ns": time.time_ns(),
                                    "monotonic_ns": time.monotonic_ns()}) + "\n")
        emissions.flush()
        prefix = f"replay-w{worker}-"
        if phase == 0:
            emit({"type": "progress", "sequence": 1, "status": 102, "published": True,
                  "summary": prefix + "progress"})
        elif phase == 1:
            tool("measured-tool", prefix + "tool", False)
        elif phase == 2:
            output({"type": "response.output_item.added", "sequence_number": 1,
                    "output_index": 0,
                    "item": {"type": "message", "id": "replay-message",
                             "status": "in_progress", "role": "assistant",
                             "content": [], "phase": "final_answer"}})
            output({"type": "response.output_text.delta", "sequence_number": 2,
                    "item_id": "replay-message", "output_index": 0,
                    "content_index": 0, "delta": prefix + "stream"})
        elif phase == 3:
            tool("measured-tool", prefix + "tool", True)
        else:
            (directory / "done").touch()
            emit({"type": "result", "sequence": 2, "status": 200,
                  "summary": prefix + "terminal", "result": {"text": prefix + "terminal"}})
