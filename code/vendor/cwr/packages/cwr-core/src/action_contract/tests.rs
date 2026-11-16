#[cfg(test)]
mod tests {
    use jsonschema::validator_for;
    use serde_json::json;

    use super::*;
    use crate::v2026_09::invocation::{ActionResponse, ActionResponseStatus};

    fn exact_target() -> Value {
        json!({"runtime_ref":"cwp://org.example/loops/one", "coworker_ref":"cwp://org.example/coworkers/writer",
            "role_ref":"cwp://org.example/roles/writing", "action_name":"draft--brief",
            "binding_ref":"cwp://org.example/coworkers/writer/bindings/main", "contract_id":"sha256:test"})
    }

    fn completed_response() -> ActionResponse {
        ActionResponse {
            progress: None,
            invocation_ref: "~@/invocations/child".into(),
            sequence: 1,
            status: ActionResponseStatus::COMPLETED,
            summary: "Drafted the brief".into(),
            result: Some(json!({"artifact_ref": "~@/artifacts/brief"})),
            error: None,
            cause: None,
        }
    }

    fn accepts(schema: &Value, instance: &Value) -> bool {
        validator_for(schema).unwrap().is_valid(instance)
    }

    #[test]
    fn historical_targets_load_but_discovery_uses_runtime_references() {
        let canonical = exact_target();
        let mut legacy = canonical.clone();
        let fields = legacy.as_object_mut().unwrap();
        let reference = fields.remove("runtime_ref").unwrap();
        fields.insert("loop_ref".into(), reference);
        let loaded: ExactActionTarget = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(loaded).unwrap(), canonical);
        assert!(accepts(&exact_action_target_schema(), &canonical));
        legacy["runtime_ref"] = canonical["runtime_ref"].clone();
        assert!(serde_json::from_value::<ExactActionTarget>(legacy).is_err());
    }

    #[test]
    fn intrinsic_runtime_operation_names_are_stable() {
        assert_eq!(START_ACTION_HOST_OPERATION, "cwr.start_action");
        assert_eq!(INVOKE_ACTION_HOST_OPERATION, "cwr.invoke_action");
        assert_eq!(STOP_INVOCATION_HOST_OPERATION, "cwr.stop_invocation");
    }

    #[test]
    fn typed_inputs_use_the_exact_wire_shapes() {
        let start: StartActionInput = serde_json::from_value(json!({
            "activation": "@writer--draft--brief",
            "input": {"topic": "launch"}}))
        .unwrap();
        assert_eq!(
            serde_json::to_value(start).unwrap(),
            json!({
                "activation": "@writer--draft--brief",
                "input": {"topic": "launch"}})
        );

        let invoke: InvokeActionInput = serde_json::from_value(json!({
            "target": exact_target(),
            "input": {"topic": "launch"}
        }))
        .unwrap();
        assert_eq!(invoke.target.action_name, "draft--brief");

        let stop: StopInvocationInput = serde_json::from_value(json!({
            "invocation_ref": "~@/invocations/child",
            "reason": "Superseded"
        }))
        .unwrap();
        assert_eq!(stop.reason.as_deref(), Some("Superseded"));
        assert!(serde_json::from_value::<StopInvocationInput>(json!({
            "invocation_ref": "~@/invocations/child",
            "reason": "  "
        }))
        .is_err());
        assert!(serde_json::from_value::<StartActionInput>(json!({
            "activation": "@writer--draft--brief",
            "input": {},
            "unexpected": true
        }))
        .is_err());
    }

    #[test]
    fn typed_results_expose_stable_refs_and_states() {
        let start = StartActionResult::admitted("~@/invocations/child");
        assert!(start.is_queued());
        assert_eq!(
            serde_json::to_value(start).unwrap(),
            json!({
                "invocation_ref": "~@/invocations/child",
                "state": "admitted",
                "queued": true
            })
        );
        assert!(serde_json::from_value::<StartActionResult>(json!({
            "invocation_ref": "~@/invocations/child",
            "state": "admitted",
            "queued": false
        }))
        .is_err());

        let stopped = StopInvocationResult::Stopped {
            invocation_ref: "~@/invocations/child".into(),
        };
        let too_late = StopInvocationResult::TooLate {
            invocation_ref: "~@/invocations/child".into(),
        };
        assert_eq!(stopped.state(), StopInvocationResultState::Stopped);
        assert_eq!(too_late.state(), StopInvocationResultState::TooLate);
        assert_eq!(stopped.invocation_ref(), too_late.invocation_ref());
    }

    #[test]
    fn exact_schemas_accept_each_typed_variant_and_reject_near_misses() {
        let cases = [
            (
                start_action_input_schema(),
                json!({
                    "activation": "@writer--draft--brief",
                    "input": {}}),
            ),
            (
                start_action_output_schema(),
                json!({
                    "invocation_ref": "~@/invocations/child",
                    "state": "admitted",
                    "queued": true
                }),
            ),
            (
                invoke_action_input_schema(),
                json!({
                    "target": exact_target(),
                    "input": {"topic": "launch"}
                }),
            ),
            (
                invoke_action_output_schema(),
                serde_json::to_value(completed_response()).unwrap(),
            ),
            (
                stop_invocation_input_schema(),
                json!({
                    "invocation_ref": "~@/invocations/child",
                    "reason": "Superseded"
                }),
            ),
            (
                stop_invocation_output_schema(),
                json!({
                    "invocation_ref": "~@/invocations/child",
                    "state": "too_late"
                }),
            ),
        ];
        for (schema, instance) in cases {
            assert!(
                accepts(&schema, &instance),
                "{instance} did not match {schema}"
            );
        }

        assert!(!accepts(
            &start_action_output_schema(),
            &json!({
                "invocation_ref": "~@/invocations/child",
                "state": "admitted",
                "queued": false
            })
        ));
        assert!(!accepts(
            &stop_invocation_input_schema(),
            &json!({
                "invocation_ref": "~@/invocations/child",
                "reason": " "
            })
        ));
        assert!(!accepts(
            &stop_invocation_output_schema(),
            &json!({
                "invocation_ref": "~@/invocations/child",
                "state": "completed"
            })
        ));
    }

    #[test]
    fn return_result_schema_accepts_exact_ranges_but_never_host_status_499() {
        let schema = return_result_schema(&json!({
            "type": "object",
            "required": ["ok"],
            "properties": {"ok": {"const": true}},
            "additionalProperties": false
        }));
        for response in [
            json!({"status": 100, "summary": "Started"}),
            json!({"status": 199, "summary": "Still working"}),
            json!({"status": 200, "summary": "Done", "result": {"ok": true}}),
            json!({"status": 299, "summary": "Done", "result": {"ok": true}}),
            json!({"status": 400, "summary": "Blocked", "error": {"code": "blocked"}}),
            json!({
                "status": 498,
                "summary": "Blocked",
                "error": {"code": "blocked", "message": "Needs approval", "details": {}}
            }),
            json!({"status": 500, "summary": "Failed", "error": {"code": "failed"}}),
            json!({"status": 599, "summary": "Failed", "error": {"code": "failed"}}),
        ] {
            assert!(accepts(&schema, &response), "{response} must be accepted");
        }
        for response in [
            json!({"status": 99, "summary": "Invalid"}),
            json!({"status": 300, "summary": "Invalid"}),
            json!({"status": 399, "summary": "Invalid"}),
            json!({"status": 499, "summary": "Stopped"}),
            json!({"status": 600, "summary": "Invalid", "error": {"code": "failed"}}),
            json!({"status": 150, "summary": "Working", "result": {}}),
            json!({"status": 202, "summary": "Done"}),
            json!({"status": 451, "summary": "Blocked"}),
            json!({"status": 503, "summary": "Failed", "error": {"code": " "}}),
            json!({
                "status": 503,
                "summary": "Failed",
                "error": {"code": "failed", "message": "   "}
            }),
        ] {
            assert!(!accepts(&schema, &response), "{response} must be rejected");
        }
    }

    #[test]
    fn invocation_schema_preserves_every_terminal_status_category() {
        let schema = invoke_action_output_schema();
        for (expected_status, response) in [
            (
                202,
                json!({
                    "invocation_ref": "~@/invocations/child", "sequence": 1,
                    "status": 202, "summary": "Accepted", "result": {}
                }),
            ),
            (
                451,
                json!({
                    "invocation_ref": "~@/invocations/child", "sequence": 1,
                    "status": 451, "summary": "Unavailable", "error": {"code": "unavailable"}
                }),
            ),
            (
                499,
                json!({
                    "invocation_ref": "~@/invocations/child", "sequence": 1,
                    "status": 499, "summary": "Stopped", "cause": {"code": "stop_requested"}
                }),
            ),
            (
                503,
                json!({
                    "invocation_ref": "~@/invocations/child", "sequence": 1,
                    "status": 503, "summary": "Unavailable", "error": {"code": "provider_error"}
                }),
            ),
        ] {
            assert!(accepts(&schema, &response), "{response} must match the invocation schema");
            let parsed: ActionResponse = serde_json::from_value(response).unwrap();
            assert_eq!(parsed.status.as_u16(), expected_status);
        }
    }
}
