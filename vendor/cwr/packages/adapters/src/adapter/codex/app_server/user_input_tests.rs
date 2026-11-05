#[cfg(test)]
mod user_input_tests {
    use super::*;
    #[derive(Default)]
    struct Host { calls: Vec<Value>, answered: bool }
    impl BoundedActionHandler for Host {
        fn on_progress(&mut self, _:u16, _:&str)->CwrResult<()> {Ok(())}
        fn on_activity(&mut self,_:AdapterActivityEvent)->CwrResult<()> {Ok(())}
        fn on_model_output(&mut self,_:AdapterModelOutput)->CwrResult<()> {Ok(())}
        fn on_host_call(&mut self,_:&str,op:&str,input:Value)->CwrResult<ToolCallResult> {
            assert_eq!(op,cwr::adapter::user_input::USER_INPUT_OPERATION);
            self.calls.push(input.clone());
            Ok(ToolCallResult::success(if input["action"] == "poll" && self.answered {
                json!({"state":"answered","response":{"answers":{"color":{"answers":["Blue"]}}}})
            } else {json!({"state":"pending"})}))
        }
    }
    fn question(id:Value)->Value {
        json!({"id":id,"method":"item/tool/requestUserInput","params":{"threadId":"thread","turnId":"turn","itemId":"item","isBlocking":true,"questions":[{"id":"color","header":"Color","question":"Which?","isOther":true,"isSecret":false,"options":[{"label":"Blue","description":"Ocean"}]}]}})
    }
    #[test]
    fn provider_question_returns_exact_rpc_id_and_answer_shape_once() {
        for id in [json!(7),json!("7")] {
            let mut host=Host::default(); let mut pending=PendingUserInputs::default(); let mut output=vec![];
            let scope=CodexOpenResponsesMapper::new("thread","turn");
            assert!(pending.observe(&question(id.clone()),Some(&scope),&mut output,&mut host).unwrap());
            pending.poll(&mut output,&mut host).unwrap();
            assert!(output.is_empty());
            host.answered=true; pending.last_poll=None;
            pending.poll(&mut output,&mut host).unwrap();
            let reply:Value=serde_json::from_slice(&output).unwrap();
            assert_eq!(reply,json!({"id":id,"result":{"answers":{"color":{"answers":["Blue"]}}}}));
            let size=output.len(); pending.last_poll=None; pending.poll(&mut output,&mut host).unwrap();
            assert_eq!(output.len(),size);
        }
    }
    #[test]
    fn provider_question_rejects_other_turn_and_handles_resolved_without_answer() {
        let mut host=Host::default(); let mut pending=PendingUserInputs::default(); let mut output=vec![];
        let scope=CodexOpenResponsesMapper::new("thread","turn");
        let mut other=question(json!(1)); other["params"]["turnId"]=json!("old");
        pending.observe(&other,Some(&scope),&mut output,&mut host).unwrap();
        assert!(host.calls.is_empty()); assert!(serde_json::from_slice::<Value>(&output).unwrap().get("error").is_some());
        output.clear();
        pending.observe(&question(json!(2)),Some(&scope),&mut output,&mut host).unwrap();
        pending.observe(&json!({"method":"serverRequest/resolved","params":{"threadId":"other","requestId":2}}),Some(&scope),&mut output,&mut host).unwrap();
        assert_eq!(pending.requests.len(),1);
        pending.observe(&json!({"method":"serverRequest/resolved","params":{"threadId":"thread","requestId":2}}),Some(&scope),&mut output,&mut host).unwrap();
        assert!(pending.requests.is_empty()); assert_eq!(host.calls.last().unwrap()["action"],"cancel");
        pending.poll(&mut output,&mut host).unwrap(); assert!(output.is_empty());
    }
}
