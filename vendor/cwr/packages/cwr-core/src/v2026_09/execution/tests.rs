use super::*;
use crate::{
    action_contract::{ResponseMode, StartActionResult},
    adapter::{
        run_process_adapter_with_host_calls, ProcessAdapterOutcome, ProcessAdapterStopControl,
        ResolvedProcessAdapter, CLAUDE_ADAPTER_REF, CODEX_ADAPTER_REF, OPEN_RESPONSES_ADAPTER_REF,
    },
    v2026_09::{
        graph::resolve_graph,
        invocation::{ActionResponse, ActionResponseStatus},
    },
};
use std::{fs, os::unix::fs::PermissionsExt};

mod legacy;

const INVOCATION_REF: &str = "cwp://example.test/invocations/caller";
const CHILD_REF: &str = "cwp://example.test/invocations/child";

fn native_graph() -> (tempfile::TempDir, ResolvedCwrGraph, String) {
    let temporary = tempfile::tempdir().unwrap();
    for (path, content) in [
        ("CWR.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './caller.toml'\nas = 'caller'\n[[imports]]\npath = './worker.toml'\nas = 'worker'\n"),
        ("ROLE.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n[[actions]]\nverb = 'echo'\nobject = 'message'\ntitle = 'Echo'\ndescription = 'Return an answer.'\n[actions.input_schema]\npath = './input.json'\n[actions.output_schema]\npath = './output.json'\n"),
        ("integration.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n"),
        ("input.json", r#"{"type":"object","additionalProperties":false}"#),
        ("output.json", r#"{"type":"object","required":["answer"],"additionalProperties":false,"properties":{"answer":{"type":"string"}}}"#),
    ] {
        fs::write(temporary.path().join(path), content).unwrap();
    }
    for name in ["caller", "worker"] {
        let native = if name == "caller" {
            "[[imports]]\npath = './worker.toml'\nas = 'worker'\n[[native_tools]]\ncoworker = '~@/worker'\naction = 'echo--message'\n"
        } else {
            ""
        };
        fs::write(temporary.path().join(format!("{name}.toml")), format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = '{name}'\nslug = '{name}'\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './integration.toml'\nas = 'local'\n{native}"
        )).unwrap();
    }
    let graph = resolve_graph(temporary.path().join("CWR.toml")).unwrap();
    let caller = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "caller")
        .unwrap()
        .actor_ref
        .clone();
    (temporary, graph, caller)
}

#[test]
fn native_names_are_checked_against_the_callers_provider_encoding() {
    for (adapter, local_length, accepted) in [
        (CLAUDE_ADAPTER_REF, 64, true),
        (CLAUDE_ADAPTER_REF, 65, false),
        (OPEN_RESPONSES_ADAPTER_REF, 59, true),
        (OPEN_RESPONSES_ADAPTER_REF, 60, false),
        (CODEX_ADAPTER_REF, 200, true),
    ] {
        let (_temporary, mut graph, caller) = native_graph();
        graph
            .actors
            .iter_mut()
            .find(|actor| actor.coworker_slug == "worker")
            .unwrap()
            .coworker_slug = "w".repeat(local_length - "--echo--message".len());
        // The selected caller's adapter controls encoding, even when the target
        // uses a different provider and has no native selections itself.
        for binding in &mut graph.actor_integrations {
            binding.adapter_ref = if binding.actor_ref == caller {
                adapter
            } else {
                OPEN_RESPONSES_ADAPTER_REF
            }
            .into();
        }
        let outcome = retain_native_tool_definitions(&mut graph);
        if accepted {
            outcome.unwrap();
        } else {
            let error = outcome.unwrap_err().to_string();
            assert!(error.contains("permits at most 64 bytes"), "{error}");
            assert!(error.contains(adapter), "{error}");
            assert!(graph
                .actor_integrations
                .iter()
                .all(|binding| binding.adapter_params.get("cwr_native_tools").is_none()));
        }
    }
}

#[test]
fn every_caller_binding_must_accept_its_native_names() {
    let (_temporary, mut graph, caller) = native_graph();
    graph
        .actors
        .iter_mut()
        .find(|actor| actor.coworker_slug == "worker")
        .unwrap()
        .coworker_slug = "w".repeat(60 - "--echo--message".len());
    let binding = graph
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_ref == caller)
        .unwrap();
    binding.adapter_ref = CLAUDE_ADAPTER_REF.into();
    let mut second = binding.clone();
    second.actor_integration_ref.push_str("_open_responses");
    second.adapter_ref = OPEN_RESPONSES_ADAPTER_REF.into();
    graph.actor_integrations.push(second.clone());
    let error = retain_native_tool_definitions(&mut graph)
        .unwrap_err()
        .to_string();
    assert!(error.contains(&second.actor_integration_ref));
    assert!(error.contains("65 bytes"));
}

#[test]
fn native_sync_and_async_envelopes_pass_the_process_call_result_boundary() {
    for mode in [ResponseMode::Sync, ResponseMode::Async] {
        let (temporary, mut graph, caller) = native_graph();
        let target_route = graph
            .routes
            .iter_mut()
            .find(|route| route.actor_ref != caller)
            .unwrap();
        target_route.response_mode = mode;
        let action_result_schema = target_route.output_schema.clone();
        let expected_target = route_action_target(&graph.runtime_ref, target_route);
        retain_native_tool_definitions(&mut graph).unwrap();
        let binding = graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_ref == caller)
            .unwrap();
        let mut invocation = binding.project_adapter_invocation(
            INVOCATION_REF,
            "echo--message",
            json!({}),
            json!({"type":"object"}),
            json!({}),
        );
        install_native_tool_definitions(&mut invocation).unwrap();
        let operation = &invocation.host_operations[0];
        assert_eq!(
            operation.input_schema["x-cwr-target"],
            serde_json::to_value(expected_target).unwrap()
        );
        let response = match mode {
            ResponseMode::Sync => serde_json::to_value(ActionResponse {
                progress: None,
                invocation_ref: CHILD_REF.into(),
                sequence: 1,
                status: ActionResponseStatus::COMPLETED,
                summary: "Answered".into(),
                result: Some(json!({"answer":"ready"})),
                error: None,
                cause: None,
            })
            .unwrap(),
            ResponseMode::Async => {
                serde_json::to_value(StartActionResult::admitted(CHILD_REF)).unwrap()
            }
        };
        // A strict child result schema cannot validate either native envelope.
        assert!(!jsonschema::validator_for(&action_result_schema)
            .unwrap()
            .is_valid(&response));
        let executable = temporary.path().join("adapter");
        let reply_path = temporary.path().join("reply.json");
        fs::write(&executable, format!(r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"native-call","operation":"{}","input":{{}}}}'
IFS= read -r reply
printf '%s' "$reply" > "$1"
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Finished","result":{{}}}}'
"##, operation.name)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut calls = 0;
        let outcome = run_process_adapter_with_host_calls(
            &ResolvedProcessAdapter::new(executable).with_argument(&reply_path),
            invocation,
            &ProcessAdapterStopControl::new(),
            |_| {},
            |call| {
                assert_eq!(call.operation, "cwp.worker--echo--message");
                calls += 1;
                Ok(response.clone())
            },
        )
        .unwrap();
        assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
        assert_eq!(calls, 1);
        let reply: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(reply_path).unwrap()).unwrap();
        assert_eq!(reply["result"], response);
        assert!(reply.get("error").is_none());
    }
}
