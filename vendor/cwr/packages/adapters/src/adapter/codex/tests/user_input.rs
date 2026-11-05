#[test]
fn codex_provider_question_round_trip_continues_the_same_turn() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*) printf '%s\n' '{"id":"cwr-initialize","result":{}}';;
*'"method":"thread/start"'*)
  case "$line" in *'provider_user_input'*) exit 41;; esac
  printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"thread"}}}';;
*'"method":"thread/inject_items"'*) printf '%s\n' '{"id":"cwr-observation-inject","result":{}}';;
*'"method":"turn/start"'*)
  printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"turn"}}}'
  printf '%s\n' '{"id":42,"method":"item/tool/requestUserInput","params":{"threadId":"thread","turnId":"turn","itemId":"question","isBlocking":true,"questions":[{"id":"color","header":"Color","question":"Which?","isOther":true,"isSecret":false,"options":null}]}}';;
*'"id":42'*'"result":{"answers":{"color":{"answers":["Blue"]}}}'*)
  printf '%s\n' '{"method":"serverRequest/resolved","params":{"threadId":"thread","requestId":42}}'
  printf '%s\n' '{"id":43,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Continued.","result":{"message":"Blue received"}}}}';;
*'"id":43'*'"result"'*)
  printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}}'
  exit 0;;
esac
done
"#;
    let (directory,codex)=fixture(script);
    let mut request=invocation(&codex);
    request.host_operations.push(cwr::adapter::user_input::host_operation());
    let mut calls=vec![];
    let result=run_process_adapter_with_host_calls(&codex_adapter(directory.path()),request,&ProcessAdapterStopControl::new(), |_| {}, |call| {
        calls.push(call.input.clone());
        assert_eq!(call.operation,cwr::adapter::user_input::USER_INPUT_OPERATION);
        Ok(if call.input["action"] == "start" {json!({"state":"pending"})}
        else {json!({"state":"answered","response":{"answers":{"color":{"answers":["Blue"]}}}})})
    }).unwrap();
    let ProcessAdapterOutcome::Completed(result)=result else {panic!("stopped")};
    assert_eq!(result.result,Some(json!({"message":"Blue received"})));
    assert_eq!(calls.len(),2);
    assert_eq!(calls[0]["key"],calls[1]["key"]);
}
