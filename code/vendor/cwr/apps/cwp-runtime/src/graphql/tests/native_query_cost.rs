use super::*;
use cwr::v2026_09::conversation::{measure_path_reads, ConversationTimelineRollout};
use cwr::v2026_09::local::LocalRuntimeOptions;

fn imported_history(runtime: &LocalRuntime, directory: &Path, count: usize) -> String {
    let caller = runtime.catalog().unwrap().acting_actor.actor_ref;
    let conversation = "cwp://example.test/cost/source";
    let root = format!("{conversation}/root");
    let message = format!("{conversation}/message");
    let progress = format!("{conversation}/progress");
    let reply = format!("{conversation}/reply");
    let children = format!("{conversation}/children");
    let unrelated = format!("{conversation}/unrelated");
    let items = (1..=count).map(|index| {
        let publication = format!("{conversation}/publications/{index:06}");
        json!({"item_ref":format!("{publication}/item"),"publication_ref":publication,
            "connection_ref":root,"sibling_sequence":index,
            "content":if index==1 || index==count {json!({"kind":"invocation","invocation_ref":format!("{conversation}/work/{index:06}")})} else {json!({"kind":"message","message_ref":message})},
            "child_connection_ref":if index==count {Some(&children)} else if index==1 {Some(&unrelated)} else {None}})
    }).collect::<Vec<_>>();
    let branch = |reference: &str, parent: usize| {
        json!({
            "connection_ref":reference,"owner":{"kind":"timeline_item","item_ref":format!("{conversation}/publications/{parent:06}/item")},
            "revision":count,"next_sequence":count+1,
            "items":(1..=count).map(|index| { let publication=format!("{reference}/publications/{index:06}");
                let source = if parent == count && (index == count - 3 || index == count - 1) {
                    &progress
                } else if parent == count && (index == count - 2 || index == count) {
                    &reply
                } else {
                    &message
                };
                json!({"item_ref":format!("{publication}/item"),"publication_ref":publication,
                    "connection_ref":reference,"sibling_sequence":index,
                    "content":{"kind":"message","message_ref":source}})
            }).collect::<Vec<_>>()
        })
    };
    let export = json!({"version":"cwr.conversation-export/3","exported_at_unix_ms":1,
        "conversation":{"conversation_ref":conversation,"participant_actor_refs":[caller],
            "messages":[
                {"message_ref":message,"author_actor_ref":caller,"addressed_actor_refs":[],"text":"Retained message"},
                {"message_ref":progress,"author_actor_ref":caller,"addressed_actor_refs":[],"text":"Work in progress",
                    "reply_to_message_ref":message,"progress":{"invocation_ref":format!("{conversation}/work/{count:06}"),
                        "source_activity_ref":format!("{conversation}/activity/progress")}},
                {"message_ref":reply,"author_actor_ref":caller,"addressed_actor_refs":[],"text":"Finished work",
                    "reply_to_message_ref":message}],
            "activity":[
                {"activity_ref":format!("{conversation}/activity/source"),"sequence":1,"occurred_at_unix_ms":1,
                    "event":{"kind":"message_recorded","message_ref":message}},
                {"activity_ref":format!("{conversation}/activity/progress"),"sequence":2,"occurred_at_unix_ms":2,
                    "event":{"kind":"message_recorded","message_ref":progress}},
                {"activity_ref":format!("{conversation}/activity/reply"),"sequence":3,"occurred_at_unix_ms":3,
                    "event":{"kind":"message_recorded","message_ref":reply}}]},
        "timeline":{"work":([1,count].map(|index|json!({
            "invocation_ref":format!("{conversation}/work/{index:06}"),"requester_actor_ref":caller,"target_actor_ref":caller,
            "action_name":"retained--work","source_message_ref":message,"reply_message_ref":null,"parent_invocation_ref":null,
            "admission_sequence":if index==1 {1} else {2},"admitted_at_unix_ms":1,"started_at_unix_ms":1,"finished_at_unix_ms":2,
            "status":cwr::v2026_09::invocation::ActionResponseStatus::COMPLETED,"summary":"Done","activities":[]
        }))),"timeline":{"root_connection_ref":root,"connections":[
            {"connection_ref":root,"owner":{"kind":"conversation","conversation_ref":conversation},
                "revision":count,"next_sequence":count+1,"items":items},
            branch(&children,count),branch(&unrelated,1)]}}
    });
    let path = directory.join("cost-export.json");
    fs::write(&path, serde_json::to_vec(&export).unwrap()).unwrap();
    let response = runtime
        .import_conversation_as(&caller, path.to_str().unwrap(), Some("native-cost"))
        .unwrap();
    let cwr::intrinsic_cwr::IntrinsicCwrResponse::ConversationImported { conversation_ref } =
        response
    else {
        panic!("unexpected import")
    };
    conversation_ref
}

fn imported_legacy_history(runtime: &LocalRuntime, directory: &Path, count: usize) -> String {
    let caller = runtime.catalog().unwrap().acting_actor.actor_ref;
    let export = json!({"version":"cwr.conversation-export/2","exported_at_unix_ms":1,
        "conversation":{"conversation_ref":"cwp://example.test/legacy-cost/source","participant_actor_refs":[caller],
        "activity":((0..count).map(|index|json!({"activity_ref":format!("cwp://example.test/legacy-cost/activity/{index:06}"),"sequence":index+1,"occurred_at_unix_ms":index+1,
            "event":{"kind":"message_recorded","message_ref":format!("cwp://example.test/legacy-cost/messages/{index:06}")}})).collect::<Vec<_>>()),
        "messages":(0..count).map(|index|json!({"message_ref":format!("cwp://example.test/legacy-cost/messages/{index:06}"),
            "author_actor_ref":caller,"addressed_actor_refs":[],"text":"Retained legacy message"})).collect::<Vec<_>>()}
    });
    let path = directory.join("legacy-cost-export.json");
    fs::write(&path, serde_json::to_vec(&export).unwrap()).unwrap();
    let response = runtime
        .import_conversation_as(&caller, path.to_str().unwrap(), Some("legacy-cost"))
        .unwrap();
    let cwr::intrinsic_cwr::IntrinsicCwrResponse::ConversationImported { conversation_ref } =
        response
    else {
        panic!("unexpected import")
    };
    conversation_ref
}

#[test]
fn compiled_native_timeline_keeps_sql_work_and_response_bounded() {
    for canonical in [true, false] {
        let mut measurements = Vec::new();
        for count in [1_201, 6_001] {
            let directory = tempfile::tempdir().unwrap();
            let runtime = mutations::fixture_with_options(
                directory.path(),
                LocalRuntimeOptions {
                    conversation_timeline_rollout: if canonical {
                        ConversationTimelineRollout::NewConversations
                    } else {
                        ConversationTimelineRollout::Disabled
                    },
                    ..Default::default()
                },
            );
            let conversation = if canonical {
                imported_history(&runtime, directory.path(), count)
            } else {
                imported_legacy_history(&runtime, directory.path(), count)
            };
            let server = start(
                runtime,
                "cwp://example.test/runtime".into(),
                directory.path(),
                crate::idle::Activity::default(),
                Arc::new(|_| Ok(())),
                tokio::sync::watch::channel(()).1,
            )
            .unwrap();
            let client = client(&server.endpoint);
            let endpoint = session_endpoint(&client, &server.endpoint, None);
            let operation = include_str!("../../../testdata/native-conversation-timeline.graphql");
            let (response, steps) =
                measure_path_reads(&directory.path().join("cwpdb.sqlite3"), || {
                    query_session(
                        &client,
                        &endpoint,
                        operation,
                        json!({"url":conversation,"before":null,"last":5}),
                    )
                });
            assert!(response.get("errors").is_none(), "{response}");
            let conversation = &response["data"]["runtime"]["conversation____ref___v_url"];
            let page = &conversation[if canonical {
                "canonicalTimeline____last___v_last____before___v_before"
            } else {
                "legacyHistory____last___v_last____before___v_before"
            }];
            assert_eq!(page["edges"].as_array().unwrap().len(), 5);
            if canonical {
                let roles = [
                    "children____filter___o_childRole__e_COMMENTARY_c____last___l_1",
                    "children____filter___o_childRole__e_FINAL_REPLY_c____last___l_1",
                ];
                let selected_children: usize = page["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|edge| {
                        roles
                            .iter()
                            .filter_map(|role| edge["node"][role]["edges"].as_array())
                    })
                    .map(Vec::len)
                    .sum();
                assert_eq!(
                    selected_children, 2,
                    "one matching child for each collapsed preview should be hydrated"
                );
            }
            assert!(
                steps > 0,
                "worker read connections must participate in measurement"
            );
            measurements.push((steps, serde_json::to_vec(&response).unwrap().len()));
        }
        eprintln!("compiled native timeline canonical={canonical} (SQLite VM steps, response bytes): {measurements:?}");
        assert!(
            measurements[1].0 < measurements[0].0 * 2,
            "fixed native page scanned retained history: {measurements:?}"
        );
        assert!(
            measurements[1].1 < measurements[0].1 + 2048,
            "fixed native response grew with retained history: {measurements:?}"
        );
    }
}
