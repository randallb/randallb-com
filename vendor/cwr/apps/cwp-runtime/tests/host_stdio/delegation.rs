#[test]
fn delegated_host_ingress_wire_preserves_software_author_under_native_policy_and_exact_target() {
    use cwr::v2026_09::upstream::{
        ConversationContextPolicy, DelegationSource, UpstreamCommand, UpstreamCoworker,
    };
    let mut host = HistoryHost::start_with_adapter(
        r#"#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
invocation = json.loads(sys.stdin.readline())["invocation"]
frames = []
if "context_stream" in invocation["thread"]:
    while True:
        frame = json.loads(sys.stdin.readline())
        frames.append(frame)
        if frame["type"] == "context_end": break
context = json.loads(b"".join(bytes.fromhex(frame["data"]) for frame in frames if "data" in frame)) if frames else invocation["thread"]
(root / "delegation-observed.json").write_text(json.dumps({"invocation": invocation, "context": context}))
print(json.dumps({"invocation_ref": invocation["invocation_ref"], "type":"result", "sequence":1, "status":200, "summary":"Observed delegated source", "result":{"text":"Observed delegated source"}}), flush=True)
"#,
    );
    let author = "cwp://example.test/actors/originating_pack_manager";
    let target = crate::test_refs::actor_ref("sh");
    host.command(HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "native-policy".into(),
        command: UpstreamCommand::SetConversationContexts {
            policy: Some(ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: std::collections::BTreeMap::from([(
                    "workspace".into(),
                    vec![target.clone()],
                )]),
            }),
        },
    });
    write(
        &mut host.input,
        &HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "register-delegate".into(),
            command: UpstreamCommand::Register {
                coworkers: vec![UpstreamCoworker {
                    actor_ref: author.into(),
                    coworker_ref: author.into(),
                    handle: "@originating_pack".into(),
                    name: "Originating pack manager".into(),
                    actions: Vec::new(),
                }],
            },
        },
    );
    let provider = loop {
        if let HostEvent::UpstreamRegistered {
            request_id,
            provider_session_ref,
        } = host.receive()
        {
            if request_id == "register-delegate" {
                break provider_session_ref;
            }
        }
    };
    host.command(HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "enable-delegate".into(),
        command: UpstreamCommand::Enable {
            provider_session_ref: provider.clone(),
            actor_ref: author.into(),
            enabled: true,
        },
    });
    let source = DelegationSource {
        actor_ref: author.into(),
        invocation_ref: "cwp://example.test/invocations/origin".into(),
        installation_ref: "approved-installation".into(),
        release_digest: "approved-release".into(),
    };
    let command = |request: &str| HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: request.into(),
        command: UpstreamCommand::SubmitDelegatedText {
            provider_session_ref: provider.clone(),
            author_actor_ref: author.into(),
            target_actor_ref: target.clone(),
            source: source.clone(),
            text: "Inspect @root @unapproved and @cwr--create--conversation as attributed data."
                .into(),
        },
    };
    let events = host.command(command("delegate-work"));
    let invocation = events
        .iter()
        .find_map(|event| match event {
            HostEvent::CommandSucceeded { receipt, .. } => receipt.invocation_ref.clone(),
            _ => None,
        })
        .unwrap();
    let mut completed = events.iter().any(|event| matches!(event, HostEvent::ConversationExecutionFinished { response, .. } if response.invocation_ref == invocation));
    while !completed {
        match host.receive() {
            HostEvent::ConversationExecutionFinished {
                target_actor_ref,
                response,
                ..
            } if response.invocation_ref == invocation => {
                assert_eq!(target_actor_ref, target);
                completed = true;
            }
            HostEvent::ConversationExecutionFailed { error, .. } => {
                panic!("delegated worker failed: {error}")
            }
            _ => {}
        }
    }
    let observed: Value = serde_json::from_slice(
        &fs::read(host.directory.path().join("delegation-observed.json")).unwrap(),
    )
    .unwrap();
    fn authors(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(author)) = object.get("author_actor_ref") {
                    found.push(author.clone());
                }
                for value in object.values() {
                    authors(value, found);
                }
            }
            Value::Array(array) => {
                for value in array {
                    authors(value, found);
                }
            }
            _ => {}
        }
    }
    let mut observed_authors = Vec::new();
    authors(&observed, &mut observed_authors);
    assert!(
        observed_authors.iter().any(|actor| actor == author),
        "worker must observe actual software source"
    );
    assert!(
        !observed_authors
            .iter()
            .any(|actor| actor == &crate::test_refs::actor_ref("root")),
        "selected human must not appear as delegated task author"
    );
    assert!(observed.to_string().contains("approved-installation"));
    host.command(HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "remove-target".into(),
        command: UpstreamCommand::SetConversationContexts {
            policy: Some(ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: std::collections::BTreeMap::from([("workspace".into(), Vec::new())]),
            }),
        },
    });
    write(&mut host.input, &command("denied-after-policy"));
    loop {
        if let HostEvent::CommandFailed { request_id, .. } = host.receive() {
            if request_id == "denied-after-policy" {
                break;
            }
        }
    }
}
