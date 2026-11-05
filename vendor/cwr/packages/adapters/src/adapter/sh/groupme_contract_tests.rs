#[test]
fn groupme_output_schema_accepts_shell_result_with_groupme_json_in_stdout() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../coworkers/groupme_read/output.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for payload in [
        json!({"ok":true,"result":{"groups":[{"id":"10","name":"Fixture"}]}}),
        json!({"ok":true,"result":{"group_id":"10","messages":[]}}),
    ] {
        let command = format!("printf '%s' '{}'", payload);
        let event = configured_command_result(json!({"command":command,"fail_on_nonzero":true}));
        let AdapterEvent::Result {
            status,
            result: Some(result),
            ..
        } = event
        else {
            panic!("expected successful Shell result");
        };
        assert_eq!(status, 200);
        assert!(validator.is_valid(&result), "{result}");
        let stdout: Value = serde_json::from_str(result["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(stdout, payload);
        assert!(
            !validator.is_valid(&payload),
            "payload is inside stdout, not the Shell result"
        );
    }
}
