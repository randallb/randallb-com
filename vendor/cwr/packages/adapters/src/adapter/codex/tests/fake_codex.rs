const FAKE_CODEX: &str = r#"#!/usr/bin/env sh
[ "$USER" = "fixture-user" ] || exit 30
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*'"name":"observe_thread"'*)
      exit 27
      ;;
    *'"method":"thread/start"'*'"features.code_mode.direct_only_tool_namespaces":["cwp"]'*'"model_reasoning_effort":"low"'*'"model":"gpt-5.6-sol"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"fake-thread"}}}'
      ;;
    *'"method":"thread/start"'*)
      exit 26
      ;;
    *'"method":"thread/resume"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"fake-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"turn-1"}}}'
      printf '%s\n' '{"method":"item/started","params":{"item":{"type":"commandExecution","id":"command-1","command":"git status --short","status":"inProgress"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"command-1","command":"git status --short","status":"completed","exitCode":1}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fake-thread","turnId":"turn-1","completedAtMs":100,"item":{"type":"agentMessage","id":"message-1","text":"Visible draft.","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fake-thread","turnId":"turn-1","completedAtMs":101,"item":{"type":"agentMessage","id":"message-2","text":"Working.","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":102,"summary":"Working."}}}'
      ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"callId":"send-message-1","namespace":"cwp","tool":"cwr--send--message","arguments":{"to":"@helper","text":"hello"}}}'
      ;;
    *'"id":3'*'"result"'*)
      printf '%s\n' '{"id":4,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"complete"}}}}'
      ;;
    *'"id":4'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fake-thread","turn":{"id":"turn-1","status":"completed"}}}'
      exit 0
      ;;
    *)
      printf 'unexpected app-server request: %s\n' "$line" >&2
      exit 29
      ;;
  esac
done
"#;
const FAKE_RESUME_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/resume"'*'"features.code_mode.direct_only_tool_namespaces":["cwp"]'*'"threadId":"prior-thread"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"prior-thread"}}}'
      ;;
    *'"method":"thread/start"'*)
      exit 23
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"resume-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Resumed.","result":{"message":"resumed"}}}}'
      ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"prior-thread","turn":{"id":"resume-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_NATIVE_TEXT_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*'"name":"cwr--return--result"'*)
      exit 71
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"native-text-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"native-text-turn"}}}'
      printf '%s\n' '{"method":"item/started","params":{"threadId":"native-text-thread","turnId":"native-text-turn","item":{"type":"agentMessage","id":"native-text-item","text":"","phase":"final_answer","memoryCitation":null}}}'
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"native-text-thread","turnId":"native-text-turn","itemId":"native-text-item","delta":"Streaming"}}'
      : > native-text-delta-emitted
      while [ ! -e native-text-delta-release ]; do sleep 0.01; done
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"native-text-thread","turnId":"native-text-turn","itemId":"native-text-item","delta":" result"}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"native-text-thread","turnId":"native-text-turn","item":{"type":"agentMessage","id":"native-text-item","text":"Streaming result","phase":"final_answer","memoryCitation":null}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"turn":{"id":"native-text-turn","status":"completed"}}}'
      ;;
  esac
done
"#;
const FAKE_NESTED_TURN_COMPLETION_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"parent-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"parent-turn"}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"child-thread","turn":{"id":"child-turn","status":"completed"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"parent-thread","turnId":"parent-turn","item":{"type":"agentMessage","id":"parent-answer","text":"Parent finished.","phase":"final_answer","memoryCitation":null}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"parent-thread","turn":{"id":"parent-turn","status":"completed"}}}'
      ;;
  esac
done
"#;
const FAKE_NATIVE_CONTEXT_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> native-context-requests.jsonl
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"native-context-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"thread/compact/start"'*)
      printf '%s\n' '{"id":"cwr-context-compact","result":{}}'
      printf '%s\n' '{"method":"turn/started","params":{"threadId":"native-context-thread","turn":{"id":"native-context-compaction-turn","status":"inProgress"}}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"native-context-thread","turnId":"native-context-compaction-turn","tokenUsage":{"total":{"totalTokens":0},"last":{"totalTokens":4096},"modelContextWindow":258400}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"native-context-thread","turnId":"native-context-compaction-turn","item":{"type":"contextCompaction","id":"native-context-preparation-compaction"}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-context-thread","turn":{"id":"native-context-compaction-turn","status":"completed"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"native-context-turn"}}}'
      if [ -e native-context-fail ]; then
        printf '%s\n' '{"method":"error","params":{"threadId":"native-context-thread","turnId":"native-context-turn","willRetry":false,"error":{"message":"Model context window exceeded after native compaction","codexErrorInfo":"contextWindowExceeded","additionalDetails":null}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-context-thread","turn":{"id":"native-context-turn","status":"failed","error":{"message":"Model context window exceeded after native compaction","codexErrorInfo":"contextWindowExceeded","additionalDetails":null}}}}'
      else
        printf '%s\n' '{"method":"item/started","params":{"threadId":"native-context-thread","turnId":"native-context-turn","item":{"type":"contextCompaction","id":"native-context-compaction"}}}'
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"native-context-thread","turnId":"native-context-turn","item":{"type":"contextCompaction","id":"native-context-compaction"}}}'
        printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"native-context-thread","turnId":"native-context-turn","tokenUsage":{"total":{"totalTokens":200000},"last":{"totalTokens":1024},"modelContextWindow":258400}}}'
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"native-context-thread","turnId":"native-context-turn","item":{"type":"agentMessage","id":"native-context-answer","phase":"final_answer","text":"Native context complete."}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-context-thread","turn":{"id":"native-context-turn","status":"completed"}}}'
      fi
      ;;
    *)
      printf 'unexpected app-server request: %s\n' "$line" >&2
      exit 72
      ;;
  esac
done
"#;
const FAKE_NUMERIC_STATUS_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"numeric-status-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"numeric-status-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":150,"summary":"Halfway."}}}'
      ;;
    *'"id":2'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":202,"summary":"Accepted and complete.","result":{"message":"complete at 202"}}}}'
      ;;
    *'"id":3'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"numeric-status-thread","turn":{"id":"numeric-status-turn","status":"completed"}}}'
      exit 0
      ;;
    *)
      printf 'unexpected app-server request: %s\n' "$line" >&2
      exit 61
      ;;
  esac
done
"#;
const FAKE_HOST_ONLY_STOP_STATUS_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"host-stop-status-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"host-stop-status-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":499,"summary":"Model tried to stop."}}}'
      ;;
    *'"id":2'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":204,"summary":"Corrected to completion.","result":{"message":"model could not stop"}}}}'
      ;;
    *'"id":3'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"host-stop-status-thread","turn":{"id":"host-stop-status-turn","status":"completed"}}}'
      exit 0
      ;;
    *)
      printf 'unexpected app-server request: %s\n' "$line" >&2
      exit 63
      ;;
  esac
done
"#;
const FAKE_BLOCKED_STATUS_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"blocked-status-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"blocked-status-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":451,"summary":"Unavailable for legal reasons.","error":{"code":"unavailable","message":"The source is restricted.","details":{"region":"US"}}}}}'
      ;;
    *'"id":2'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"blocked-status-thread","turn":{"id":"blocked-status-turn","status":"completed"}}}'
      exit 0
      ;;
    *)
      printf 'unexpected app-server request: %s\n' "$line" >&2
      exit 64
      ;;
  esac
done
"#;
const FAKE_SLOW_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"slow-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"slow-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":102,"summary":"Waiting."}}}'
      ;;
    *'"id":2'*'"result"'*)
      while true; do sleep 1; done
      ;;
  esac
done
"#;
const FAKE_UNFINISHED_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"unfinished-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"method":"turn/started","params":{"threadId":"unfinished-thread","turn":{"id":"turn-unfinished"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"unfinished-thread","turnId":"turn-unfinished","completedAtMs":100,"item":{"type":"agentMessage","id":"message-unfinished","text":"I was still working.","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"turn":{"id":"turn-unfinished","status":"completed"}}}'
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"turn-unfinished"}}}'
      ;;
  esac
done
"#;
const FAKE_ACTIVITY_GATE_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"gated-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      touch turn-started
      exit 31
      ;;
  esac
done
"#;
const FAKE_COMMENTARY_GATE_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"commentary-gate-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      : > commentary-reached-adapter
      printf '%s\n' '{"method":"turn/started","params":{"threadId":"commentary-gate-thread","turn":{"id":"commentary-gate-turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"another-thread","turnId":"commentary-gate-turn","completedAtMs":80,"item":{"type":"agentMessage","id":"foreign-thread-item","text":"Wrong thread.","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"commentary-gate-thread","turnId":"older-turn","completedAtMs":85,"item":{"type":"agentMessage","id":"stale-turn-item","text":"Wrong turn.","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"method":"item/started","params":{"threadId":"commentary-gate-thread","turnId":"commentary-gate-turn","startedAtMs":90,"item":{"type":"agentMessage","id":"commentary-gate-item","text":"","phase":"commentary","memoryCitation":null}}}'
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"commentary-gate-thread","turnId":"commentary-gate-turn","itemId":"commentary-gate-item","delta":"Visible before completion."}}'
      : > commentary-delta-emitted
      while [ ! -f commentary-delta-release ]; do /bin/sleep 0.02; done
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"commentary-gate-thread","turnId":"commentary-gate-turn","completedAtMs":100,"item":{"type":"agentMessage","id":"commentary-gate-item","text":"Visible before completion.","memoryCitation":null}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"complete"}}}}'
      ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"commentary-gate-turn"}}}'
      while [ ! -f commentary-release ]; do /bin/sleep 0.02; done
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"commentary-gate-thread","turn":{"id":"commentary-gate-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_CALLER_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"caller-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"caller-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"callId":"send-message-2","namespace":"cwp","tool":"cwr--send--message","arguments":{"to":"@helper","text":"question from caller"}}}'
      ;;
    *'"id":2'*'"result"'*'reply from helper'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Exchanged messages.","result":{"message":"caller received helper reply"}}}}'
      ;;
    *'"id":2'*'"result"'*)
      exit 24
      ;;
    *'"id":3'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"caller-thread","turn":{"id":"caller-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_HELPER_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"helper-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"helper-turn"}}}'
      printf '%s\n' '{"id":12,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Replied.","result":{"message":"reply from helper"}}}}'
      ;;
    *'"id":12'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"helper-thread","turn":{"id":"helper-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_ACTION_CALLER_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"action-caller-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"action-caller-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"callId":"roll-dice-1","namespace":"cwp","tool":"cwr--invoke--action","arguments":{"activation":"@rng--roll--dice","input":{"count":1,"sides":20,"modifier":0}}}}'
      ;;
    *'"id":2'*'"result"'*'\"total\":14'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Reacted to the roll.","result":{"message":"caller continued after rolling 14"}}}}'
      ;;
    *'"id":2'*'"result"'*)
      exit 25
      ;;
    *'"id":3'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"action-caller-thread","turn":{"id":"action-caller-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_GENERIC_HOST_TOOLS_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*'"name":"cwr--start--action"'*'"name":"cwr--debug--invocation"'*'"name":"cwr--stop--invocation"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"generic-tools-thread"}}}'
      ;;
    *'"method":"thread/start"'*)
      exit 41
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"generic-tools-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"callId":"start-1","namespace":"cwp","tool":"cwr--start--action","arguments":{"action_name":"draft--brief"}}}'
      ;;
    *'"id":2'*'child-1'*'"success":true'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"callId":"await-1","namespace":"cwp","tool":"cwr--debug--invocation","arguments":{"invocation_ref":"child-1"}}}'
      ;;
    *'"id":3'*'status'*'"success":true'*)
      printf '%s\n' '{"id":4,"method":"item/tool/call","params":{"callId":"stop-1","namespace":"cwp","tool":"cwr--stop--invocation","arguments":{"invocation_ref":"child-2"}}}'
      ;;
    *'"id":4'*'stopped'*'"success":true'*)
      printf '%s\n' '{"id":5,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Used generic host tools.","result":{"message":"complete"}}}}'
      ;;
    *'"id":5'*'"success":true'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"generic-tools-thread","turn":{"id":"generic-tools-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
const FAKE_GENERIC_TOOL_ERRORS_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"generic-error-thread"}}}'
      ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"generic-error-turn"}}}'
      printf '%s\n' '{"id":10,"method":"item/tool/call","params":{"callId":"invalid-1","namespace":"cwp","tool":"cwr--start--action","arguments":{}}}'
      ;;
    *'"id":10'*'"success":false'*)
      printf '%s\n' '{"id":11,"method":"item/tool/call","params":{"callId":"rejected-1","namespace":"cwp","tool":"cwr--start--action","arguments":{"action_name":"draft--brief"}}}'
      ;;
    *'"id":10'*)
      exit 42
      ;;
    *'"id":11'*'host_call_failed'*'host rejected start'*'"success":false'*)
      printf '%s\n' '{"id":12,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Observed provider errors.","result":{"message":"complete"}}}}'
      ;;
    *'"id":11'*)
      exit 43
      ;;
    *'"id":12'*'"success":true'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"generic-error-thread","turn":{"id":"generic-error-turn","status":"completed"}}}'
      exit 0
      ;;
  esac
done
"#;
