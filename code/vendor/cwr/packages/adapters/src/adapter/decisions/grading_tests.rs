use super::*;
fn input() -> Value {
    serde_json::from_str(include_str!("testdata/grading-supported.json")).unwrap()
}
fn configure(i: &mut AdapterInvocation, input: Value) {
    i.input = input;
    i.adapter_params = json!({"model":"test/decision","action_name":"grade--result","contract":"grader-v1","timeout_seconds":1});
    i.output_schema = serde_json::from_str(include_str!(
        "../../../../../packages/cwr-core/contracts/cwr/actions/2026-09/grade-result.output.schema.json"
    ))
    .unwrap();
}
fn answer(score: i64, category: bool) -> Value {
    let mut probabilities = json!({"-3":0,"-2":0,"-1":0,"0":0,"1":0,"2":0,"3":0});
    probabilities[score.to_string()] = json!(1);
    let mut r = json!({"model":"test/decision","usage":{"input_tokens":42,"output_tokens":0},"answers":{"grade":{"type":"choice","choice":score.to_string(),"confidence":1,"probabilities":probabilities}}});
    if category {
        r["answers"]["category"] = json!({"type":"choice","choice":"supported","confidence":1,"probabilities":{"supported":1,"contradiction":0,"missing_evidence":0,"unavailable_access":0}});
    }
    r
}
#[test]
fn discrete_grades_include_ungradable_without_averaging() {
    for score in -3..=3 {
        let (terminal, request) =
            run_configured(answer(score, true).to_string(), 200, false, |i| {
                configure(i, input())
            });
        assert_eq!(terminal["status"], 200, "{terminal}");
        assert_eq!(terminal["result"]["score"], score);
        assert_eq!(terminal["result"]["category"], "supported");
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["state"], json!({"context":input()["context"]}));
        assert_eq!(body["questions"]["grade"]["type"], "choice");
        assert!(body["questions"]["grade"]["criteria"]["0"]
            .as_str()
            .unwrap()
            .contains("Ungradable only"));
    }
}
#[test]
fn caller_changes_criteria_and_rubric_hash_without_changing_role() {
    let mut hashes = Vec::new();
    for criterion in ["Evidence fidelity", "Answer brevity"] {
        let mut input = input();
        input["rubric"]["criterion"] = json!(criterion);
        input["rubric"]
            .as_object_mut()
            .unwrap()
            .remove("categories");
        let (terminal, request) = run_configured(answer(3, false).to_string(), 200, false, |i| {
            configure(i, input)
        });
        assert_eq!(terminal["status"], 200, "{terminal}");
        assert!(terminal["result"]["category"].is_null());
        hashes.push(terminal["result"]["rubric"]["sha256"].clone());
        assert!(request.contains(criterion));
    }
    assert_ne!(hashes[0], hashes[1]);
}
#[test]
fn invalid_rubrics_fail_before_transport() {
    for field in ["grades", "criterion", "ungradable_when"] {
        let mut input = input();
        input["rubric"].as_object_mut().unwrap().remove(field);
        let mut i = invocation("http://127.0.0.1:1/decisions");
        configure(&mut i, input);
        let process = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-decisions"));
        let outcome =
            run_process_adapter(&process, i, &ProcessAdapterStopControl::new(), |_| {}).unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else {
            panic!("not completed")
        };
        let terminal = serde_json::to_value(terminal).unwrap();
        assert_eq!(terminal["status"], 422);
        assert!(terminal["result"].is_null());
    }
}
#[test]
fn provider_failures_and_invalid_categories_never_become_zero() {
    let mut bad = answer(0, true);
    bad["answers"]["category"]["choice"] = json!("invented");
    for (body, status) in [(bad, 200), (json!({"error":"unavailable"}), 503)] {
        let (terminal, _) =
            run_configured(body.to_string(), status, false, |i| configure(i, input()));
        assert_ne!(terminal["status"], 200);
        assert!(terminal["result"].is_null());
    }
}
#[test]
fn invocation_grading_pack_bundles_the_shared_contract() {
    for (shared, bundled) in [
        (include_str!("../../../../../packages/cwr-core/contracts/cwr/actions/2026-09/grade-result.input.schema.json"), include_str!("../../../../../wlpacks/invocation-grading/source/coworkers/grader/input.schema.json")),
        (include_str!("../../../../../packages/cwr-core/contracts/cwr/actions/2026-09/grade-result.output.schema.json"), include_str!("../../../../../wlpacks/invocation-grading/source/coworkers/grader/output.schema.json")),
    ] {
        assert_eq!(serde_json::from_str::<Value>(shared).unwrap(), serde_json::from_str::<Value>(bundled).unwrap());
    }
}
