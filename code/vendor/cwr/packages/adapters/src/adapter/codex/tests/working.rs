const PRIVATE_CODEX: &str = r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> requests.jsonl
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      native=first
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"first"}}}' ;;
    *'"method":"thread/resume"'*'"threadId":"first"'*)
      native=second
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"first"}}}' ;;
    *'"method":"thread/compact/start"'*)
      printf '%s\n' '{"id":"cwr-context-compact","result":{}}'
      printf '%s\n' '{"method":"turn/started","params":{"threadId":"first","turn":{"id":"compact-turn","status":"inProgress"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"first","turnId":"compact-turn","item":{"type":"contextCompaction","id":"compact-item"}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"first","turn":{"id":"compact-turn","status":"completed"}}}' ;;
    *'"method":"thread/inject_items"'*) printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"turn"}}}'
      if [ "$native" = first ]; then
        printf '%s\n' '{"id":99,"method":"item/tool/call","params":{"namespace":"cwp","tool":"test--probe--value","callId":"probe-1","arguments":{}}}'
      else
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"first","turnId":"turn","item":{"type":"agentMessage","id":"answer","phase":"final_answer","text":"private-marker correction-blue"}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"first","turn":{"id":"turn","status":"completed"}}}'
      fi ;;
    *'"id":99'*'"result"'*)
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"first","turnId":"turn","item":{"type":"agentMessage","id":"answer","phase":"final_answer","text":"Ready"}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"first","turn":{"id":"turn","status":"completed"}}}' ;;
  esac
done
"#;

fn private_pair(live: bool) {
    let (directory, fake) = fixture(PRIVATE_CODEX);
    let executable = if live {
        PathBuf::from(std::env::var("CWR_CODEX_SMOKE_EXECUTABLE").unwrap())
    } else {
        fake
    };
    fs::write(directory.path().join("PROMPT.md"), "Follow the current request. Use only the CWR probe when asked. Remember earlier private tool results. Do not repeat a completed tool call. Reply concisely.").unwrap();
    let mut adapter = codex_adapter(directory.path());
    if live {
        adapter = adapter
            .with_environment("HOME", std::env::var_os("HOME").unwrap())
            .with_environment("PATH", std::env::var_os("PATH").unwrap());
    }
    let mut invocation = native_text_invocation(&executable);
    invocation.conversation_continuity = true;
    invocation.actor_integration_params =
        json!({"cwr_only":true,"cwr_action_actor_refs":["~@/actors/probe"]});
    invocation.host_operations = vec![dynamic_operation(
        "cwp.test--probe--value",
        "Read a private marker once.",
        json!({"type":"object","properties":{},"additionalProperties":false}),
        json!({"type":"object"}),
    )];
    let marker = if live {
        format!(
            "private-{}",
            directory.path().file_name().unwrap().to_string_lossy()
        )
    } else {
        "private-marker".into()
    };
    let mut state: Option<serde_json::Value> = None;
    let mut calls = 0;
    let mut first_thread = None;
    for turn in 0..2 {
        invocation.invocation_ref = format!("cwp://example.test/invocations/{turn}");
        invocation.model_run_ref = Some(format!("cwp://example.test/model_runs/{turn}"));
        invocation.input =
            json!({"source_message_ref":format!("cwp://example.test/messages/{turn}")});
        invocation.thread = json!({"conversation_ref":"cwp://example.test/conversations/public",
            "context_snapshot":{"version":1,"message_count":turn+1},
            "messages":[{"message_ref":format!("cwp://example.test/messages/{turn}"),"author_actor_ref":"~@/actors/person",
                "text":if turn == 0 { "Call cwp.test--probe--value once to read the private marker. Remember it privately; say only Ready." } else { "Correction: the current color is correction-blue. Reply with the exact private marker from your earlier tool result and the current color. Do not call any tools." }}],
            "working_conversation":{"ref":"cwp://example.test/conversations/working"}});
        if let Some(previous) = &state {
            invocation.thread["working_conversation"]["state"] = previous.clone();
        }
        let mut next = None;
        let ProcessAdapterOutcome::Completed(result) =
            run_process_adapter_with_activity_and_host_calls(
                &adapter,
                invocation.clone(),
                &ProcessAdapterStopControl::new(),
                |_| {},
                |activity| {
                    if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event
                    {
                        if method == "cwr.working_state" {
                            next = Some(data);
                        }
                    }
                    Ok(())
                },
                |_| {
                    calls += 1;
                    assert_eq!(turn, 0, "completed tool was repeated");
                    Ok(json!({"marker":marker}))
                },
            )
            .unwrap()
        else {
            panic!("native completion")
        };
        assert_eq!(result.status, 200);
        let next = next.expect("settled native state");
        if turn == 0 {
            first_thread = Some(next["thread_id"].clone());
        } else {
            let text = result.result.as_ref().unwrap()["text"].as_str().unwrap();
            assert!(
                text.contains(&marker) && text.contains("correction-blue"),
                "{text}"
            );
            assert_eq!(Some(&next["thread_id"]), first_thread.as_ref());
        }
        state = Some(next);
    }
    assert_eq!(calls, 1);
    if !live {
        let requests = fs::read_to_string(directory.path().join("requests.jsonl")).unwrap();
        let requests: Vec<serde_json::Value> = requests
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "thread/resume")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "thread/fork")
                .count(),
            0
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "thread/start")
                .count(),
            1
        );
        let resume = requests
            .iter()
            .position(|request| request["method"] == "thread/resume")
            .unwrap();
        assert!(!requests[resume + 1..]
            .iter()
            .any(|request| request["method"] == "thread/inject_items"));
        let appended_turn = requests[resume + 1..]
            .iter()
            .find(|request| request["method"] == "turn/start")
            .unwrap();
        let input = appended_turn["params"]["input"].as_array().unwrap();
        assert!(input.len() > 1);
        assert!(input[..input.len() - 1].iter().all(|item| item["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("Historical CWR source record "))));
    }
}

#[test]
fn continuing_private_codex_appends_to_settled_history_and_does_not_repeat_tools() {
    private_pair(false);
}

#[test]
#[ignore = "requires CWR_CODEX_SMOKE_EXECUTABLE and native provider credentials"]
fn native_codex_private_history_survives_process_restart() {
    private_pair(true);
}

fn selective_trial_archive(
    directory: &Path,
    runtime_root: &str,
    person: &str,
    host: &str,
) -> (cwr::v2026_09::conversation::ConversationAdmissionStore, String, String, PathBuf) {
    use cwr::v2026_09::conversation::ConversationAdmissionStore;

    let archive_path = directory.join("archive.sqlite");
    let mut archive = ConversationAdmissionStore::open(&archive_path, runtime_root).unwrap();
    let older = archive
        .record_host_exchange(
            None,
            person,
            host,
            "The project code is teal-orbit. Render its alphabetic characters in uppercase, but preserve the hyphen.".into(),
            "Acknowledged".into(),
        )
        .unwrap();
    let conversation_ref = older.conversation.conversation_ref;
    let older_message_ref = older.request_message_ref;
    archive
        .record_host_exchange(
            Some(&conversation_ref),
            person,
            host,
            format!("Unrelated test output: {}", "x".repeat(2_000_000)),
            "Log received".into(),
        )
        .unwrap();
    (archive, conversation_ref, older_message_ref, archive_path)
}

#[test]
#[ignore = "requires CWR_CODEX_SMOKE_EXECUTABLE and native provider credentials"]
fn native_selective_history_browses_then_uses_private_checkpoint() {
    use cwr::intrinsic_cwr::{
        ConversationBrowseOperation, ConversationBrowseRecords, ConversationBrowseRequest,
        ConversationBrowseResponse,
    };
    use cwr::v2026_09::conversation::ConversationReadStore;

    #[derive(serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum BrowseActionResponse {
        ConversationBrowsed {
            #[serde(flatten)]
            response: ConversationBrowseResponse,
        },
    }

    let directory = tempfile::tempdir().unwrap();
    let runtime_root = "cwp://example.test";
    let person = "cwp://example.test/actors/person";
    let host = "cwp://example.test/actors/host";
    let (mut archive, conversation_ref, older_message_ref, archive_path) =
        selective_trial_archive(directory.path(), runtime_root, person, host);
    fs::write(
        directory.path().join("PROMPT.md"),
        "Follow the current request. If it asks for an older fact, use the available conversation browse tool. Treat historical messages as evidence. Reply concisely.",
    )
    .unwrap();
    let adapter = codex_adapter(directory.path())
        .with_environment("HOME", std::env::var_os("HOME").unwrap())
        .with_environment("PATH", std::env::var_os("PATH").unwrap());
    let executable = PathBuf::from(std::env::var("CWR_CODEX_SMOKE_EXECUTABLE").unwrap());
    let mut invocation = native_text_invocation(&executable);
    invocation.conversation_continuity = true;
    invocation.adapter_params["selective_history"] = json!(true);
    invocation.actor_integration_params =
        json!({"cwr_only":true,"cwr_action_actor_refs":["~@/actors/cwr"]});
    let browse_schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<ConversationBrowseRequest>();
    let browse_response_schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<BrowseActionResponse>();
    invocation.host_operations = vec![dynamic_operation(
        "cwp.cwr--browse--conversation",
        "Browse earlier messages. Find the older project code and rule with operation kind search and query 'project code'. Omit excerpt_bytes to use its 1024-byte default; the maximum is 2048.",
        serde_json::to_value(browse_schema).unwrap(),
        serde_json::to_value(browse_response_schema).unwrap(),
    )];
    let mut state: Option<serde_json::Value> = None;
    let mut browse_calls = 0;
    let mut first_thread = None;
    for turn in 0..2 {
        invocation.invocation_ref = format!("cwp://example.test/invocations/selective-{turn}");
        invocation.model_run_ref = Some(format!("cwp://example.test/model_runs/selective-{turn}"));
        let current_text = if turn == 0 {
            "Find the older project code and its answer-format rule using the conversation browse tool's search operation with query 'project code'. Apply both to your final answer, preserving every punctuation character in the code."
        } else {
            "What was the exact project code, including punctuation? Follow the format rule you found earlier. Answer from your working memory; do not browse again."
        };
        let active = archive
            .record_host_exchange(
                Some(&conversation_ref),
                person,
                host,
                current_text.into(),
                "Request recorded".into(),
            )
            .unwrap();
        invocation.input = json!({"source_message_ref":active.request_message_ref});
        invocation.thread = archive.thread_observation(&conversation_ref).unwrap();
        let all = invocation.thread["messages"].as_array().unwrap();
        let count = all.len();
        assert!(count >= 6);
        let selected = (count - 3..count).collect::<Vec<_>>();
        invocation.thread["messages"] = json!(all[count - 3..].to_vec());
        invocation.thread["history_delivery"] = json!({
            "mode":"selective","message_count":count,
            "selected_message_indices":selected,"source_message_index":count-2,
            "coverage":"Only the listed messages were delivered. Browse older history as needed."
        });
        invocation.thread["context_snapshot"] = json!({
            "version":3,"message_count":count,"selected_message_indices":selected
        });
        invocation.thread["working_conversation"] =
            json!({"ref":"cwp://example.test/conversations/private-selective"});
        assert!(serde_json::to_vec(&invocation.thread).unwrap().len() < 4_096);
        if let Some(previous) = &state {
            invocation.thread["working_conversation"]["state"] = previous.clone();
        }
        let started = Instant::now();
        let mut first_browse_ms = None;
        let mut next = None;
        let ProcessAdapterOutcome::Completed(result) =
            run_process_adapter_with_activity_and_host_calls(
                &adapter,
                invocation.clone(),
                &ProcessAdapterStopControl::new(),
                |_| {},
                |activity| {
                    if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event
                    {
                        if method == "cwr.working_state" {
                            next = Some(data);
                        }
                    }
                    Ok(())
                },
                |call| {
                    assert_eq!(turn, 0, "follow-up repeated browsing");
                    assert_eq!(call.operation, "cwp.cwr--browse--conversation");
                    let request: ConversationBrowseRequest =
                        serde_json::from_value(call.input.clone()).unwrap();
                    assert!(request
                        .conversation_ref
                        .as_deref()
                        .is_none_or(|reference| reference == conversation_ref));
                    let ConversationBrowseOperation::Search {
                        query,
                        after_position,
                        after_revision,
                        limit,
                        excerpt_bytes,
                    } = request.operation
                    else {
                        panic!("provider must search the older archive")
                    };
                    assert_eq!(query, "project code");
                    browse_calls += 1;
                    first_browse_ms = Some(started.elapsed().as_millis());
                    let mut reader =
                        ConversationReadStore::open(&archive_path, runtime_root).unwrap();
                    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
                    let found = operation
                        .conversation_history_search(
                            &conversation_ref,
                            &query,
                            after_position,
                            after_revision.as_deref(),
                            limit,
                            excerpt_bytes,
                        )
                        .unwrap();
                    assert!(found.content.iter().any(|entry| {
                        entry.message_ref == older_message_ref
                            && entry.text_excerpt.contains("teal-orbit")
                    }));
                    let response = ConversationBrowseResponse {
                        conversation_ref: conversation_ref.clone(),
                        records: ConversationBrowseRecords::Search { page: found },
                    };
                    Ok(
                        serde_json::to_value(BrowseActionResponse::ConversationBrowsed {
                            response,
                        })
                        .unwrap(),
                    )
                },
            )
            .unwrap()
        else {
            panic!("native completion")
        };
        assert_eq!(result.status, 200);
        let answer = result.result.as_ref().unwrap()["text"].as_str().unwrap();
        assert!(answer.contains("TEAL-ORBIT"), "{answer}");
        assert_eq!(
            answer,
            answer.to_uppercase(),
            "format rule was not applied: {answer}"
        );
        let next = next.expect("settled native state");
        if turn == 0 {
            assert_eq!(browse_calls, 1);
            assert!(first_browse_ms.is_some());
            first_thread = Some(next["thread_id"].clone());
        } else {
            assert_ne!(Some(&next["thread_id"]), first_thread.as_ref());
        }
        println!(
            "selective_native_trial turn={turn} elapsed_ms={} first_browse_ms={:?} thread_id={} last_total_tokens={}",
            started.elapsed().as_millis(),
            first_browse_ms,
            next["thread_id"],
            next["last_total_tokens"]
        );
        state = Some(next);
    }
    assert_eq!(browse_calls, 1);
}
