#!/usr/bin/env python3
"""Synthetic model output under the real adapter protocol, held for a GraphQL read."""
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

invocation = json.loads(sys.stdin.readline())["invocation"]
if "context_stream" in invocation["thread"]:
    while json.loads(sys.stdin.readline())["type"] != "context_end":
        pass
reference = invocation["invocation_ref"]
run = invocation["model_run_ref"]

def tool(sequence, item_id, completed=False, output_text=""):
    print(json.dumps({"type": "activity", "invocation_ref": reference, "sequence": sequence,
        "event": {"kind": "provider_event", "provider": "codex",
                  "method": "item/completed" if completed else "item/started",
                  "data": {"item": {"id": item_id, "type": "commandExecution",
                    "status": "completed" if completed else "inProgress",
                    "progress_summary": "x" * 1024, "command": "echo fixture",
                    "aggregatedOutput": output_text, "exitCode": 0}}}}), flush=True)
    acknowledgment = json.loads(sys.stdin.readline())
    assert acknowledgment["type"] == "activity_result" and not acknowledgment.get("error")

tool(1, "tool-one")
tool(2, "tool-two")

def output(event):
    print(json.dumps({"type": "model_output", "invocation_ref": reference,
        "output": {"model_run_ref": run, "provider": "fixture", "response_or_turn_id": "turn",
                   "phase": "final_answer", "event": event}}), flush=True)

output({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0,
        "item": {"type": "message", "id": "item", "status": "in_progress", "role": "assistant",
                 "content": [], "phase": "final_answer"}})
output({"type": "response.output_text.delta", "sequence_number": 2, "item_id": "item",
        "output_index": 0, "content_index": 0, "delta": "Partial answer"})
deadline = time.monotonic() + 25
while not Path(__file__).with_name("finish").exists():
    assert time.monotonic() < deadline
    time.sleep(0.01)
tool(3, "tool-one", True, "🦀" * 5000)
print(json.dumps({"type": "activity", "invocation_ref": reference, "sequence": 4,
    "event": {"kind": "provider_event", "provider": "codex", "method": "item/completed",
              "data": {"item": {"id": "file-one", "type": "fileChange", "status": "completed",
                       "changes": [{"path": "fixture.rs", "kind": "update", "diff": "+fixture"}]}}}}), flush=True)
acknowledgment = json.loads(sys.stdin.readline())
assert acknowledgment["type"] == "activity_result" and not acknowledgment.get("error")
artifact_path = Path.cwd().joinpath("chart.html")
artifact_path.write_text("<p>Streaming result chart</p>")
final_text = "Final answer\n\n\ue200visualize\ue202" + json.dumps({"path": str(artifact_path), "title": "Result chart"}) + "\ue201"
print(json.dumps({"type": "result", "invocation_ref": reference, "sequence": 1,
                  "status": 200, "summary": final_text, "result": {"text": final_text}}), flush=True)
