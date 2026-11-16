#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn internal_request_identity_is_bounded_for_long_workspace_refs() {
        let identity = format!("cwp://org.workloop/workspaces/{}/runtime/invocations/{}", "a".repeat(64), "b".repeat(32));
        let first = internal_request_id("admit", &identity);
        assert!(first.len() < 128);
        assert_eq!(first, internal_request_id("admit", &identity));
        assert_ne!(first, internal_request_id("assign", &identity));
        assert_ne!(first, internal_request_id("admit", &(identity + "x")));
    }
    #[test]
    fn assignment_hmac_matches_rfc4231() {
        assert_eq!(
            hex::encode(hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::v2026_09::invocation::{
        ActionInvocationLedger, InvocationOrigin, StartActionInvocation, StartInvocationOutcome,
    };
    use cwpdb_store::invocation::{EventKind, Receipt};
    use cwpdb_store::{CwpdbResult, NativeStore};
    struct CurrentAuthority;
    impl Authorizer for CurrentAuthority {
        fn authorize(&self, context: &Context, _: &str, _: bool) -> CwpdbResult<()> {
            if context.workspace_ref == "cwp://fixture.test/workspaces/one"
                && context.caller_ref == "cwp://fixture.test/actors/coordinator"
            {
                Ok(())
            } else {
                Err("Access denied".into())
            }
        }
    }
    fn context() -> Context {
        Context {
            workspace_ref: "cwp://fixture.test/workspaces/one".into(),
            caller_ref: "cwp://fixture.test/actors/coordinator".into(),
        }
    }
    fn receipt(outcome: Outcome) -> Receipt {
        let Outcome::Committed(receipt) = outcome else {
            panic!("unexpected expired receipt")
        };
        receipt
    }
    fn admission(
        reference: &str,
    ) -> (
        crate::v2026_09::invocation::ActionInvocationRecord,
        AdapterInvocation,
    ) {
        let mut ledger = ActionInvocationLedger::new();
        let request = StartActionInvocation {
            invocation_ref: reference.into(),
            origin: InvocationOrigin::Ingress {
                integration_ref: "cwp://fixture.test/integrations/test".into(),
                provider_event_ref: reference.into(),
            },
            requester_actor_ref: context().caller_ref,
            target_actor_ref: "cwp://fixture.test/actors/motherboard".into(),
            conversation_ref: Some("cwp://fixture.test/conversations/one".into()),
            actor_integration_ref: Some("cwp://fixture.test/bindings/motherboard".into()),
            action_name: "fixture--read".into(),
            input: json!({"fictional":true}),
            parent_invocation_ref: None,
            admitted_at_unix_ms: 1,
        };
        let started = ledger.start(request).unwrap();
        let record = match started {
            StartInvocationOutcome::Started(record) | StartInvocationOutcome::Existing(record) => {
                record
            }
        };
        let mut invocation = AdapterInvocation::new(
            reference,
            "fixture--read",
            json!({"fictional":true}),
            json!({"type":"object"}),
        );
        invocation.thread =
            json!({"conversation_ref":"cwp://fixture.test/conversations/one","messages":[]});
        (record, invocation)
    }
    #[test]
    fn assignment_results_survive_reopen_and_cannot_publish_from_another_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cwpdb.db");
        let mut coordinator = CloudCoordinator::new(
            NativeStore::open(&path).unwrap(),
            CurrentAuthority,
            vec![7; 32],
        )
        .unwrap();
        let (record, invocation) = admission("cwp://fixture.test/invocations/one");
        let admitted = receipt(
            coordinator
                .admit(context(), &record, &invocation, "admit-one".into(), 10)
                .unwrap(),
        );
        let assigned = receipt(
            coordinator
                .assign(
                    context(),
                    record.invocation_ref.clone(),
                    admitted.revision,
                    Assignment {
                        executor_ref: "cwp://fixture.test/executors/a".into(),
                        assignment_ref: "cwp://fixture.test/assignments/a".into(),
                        generation: 1,
                        route_fingerprint: "route-one".into(),
                        expires_at_unix_ms: 1000,
                    },
                    "assign-one".into(),
                    10,
                )
                .unwrap(),
        );
        let token = coordinator
            .assignment_token(&context(), &record.invocation_ref)
            .unwrap();
        let frame = json!({"type":"result","invocation_ref":record.invocation_ref,"sequence":1,"status":200,"summary":"Read fixture.","result":{"fictional":true}});
        assert!(coordinator
            .publish(
                context(),
                record.invocation_ref.clone(),
                "forged",
                assigned.revision.clone(),
                CloudCallback {
                    request_id: "terminal".into(),
                    sequence: 0,
                    frame: frame.clone()
                },
                20
            )
            .is_err());
        let terminal = coordinator
            .publish(
                context(),
                record.invocation_ref.clone(),
                &token,
                assigned.revision.clone(),
                CloudCallback {
                    request_id: "terminal".into(),
                    sequence: 0,
                    frame: frame.clone(),
                },
                20,
            )
            .unwrap();
        assert_eq!(
            receipt(terminal.clone()).terminal,
            Some(EventKind::Completed)
        );
        drop(coordinator);
        let mut reopened = CloudCoordinator::new(
            NativeStore::open(&path).unwrap(),
            CurrentAuthority,
            vec![7; 32],
        )
        .unwrap();
        assert_eq!(
            reopened
                .publish(
                    context(),
                    record.invocation_ref.clone(),
                    &token,
                    assigned.revision,
                    CloudCallback {
                        request_id: "terminal".into(),
                        sequence: 0,
                        frame
                    },
                    20
                )
                .unwrap(),
            terminal
        );
        let mut foreign = context();
        foreign.workspace_ref = "cwp://fixture.test/workspaces/other".into();
        assert!(reopened
            .assignment_token(&foreign, &record.invocation_ref)
            .is_err());
        let persisted = reopened.record(&context(), &record.invocation_ref).unwrap();
        assert_eq!(
            persisted.admitted["adapter_invocation"]["thread"],
            invocation.thread
        );
        assert!(reopened
            .assign(
                context(),
                record.invocation_ref,
                persisted.revision,
                Assignment {
                    executor_ref: "cwp://fixture.test/executors/b".into(),
                    assignment_ref: "cwp://fixture.test/assignments/b".into(),
                    generation: 2,
                    route_fingerprint: "route-one".into(),
                    expires_at_unix_ms: 1000
                },
                "reassign".into(),
                21
            )
            .is_err());
    }
    #[test]
    fn interruption_retains_uncertainty_and_denies_stale_callback() {
        let mut coordinator = CloudCoordinator::new(
            NativeStore::open_in_memory().unwrap(),
            CurrentAuthority,
            vec![8; 32],
        )
        .unwrap();
        let (record, invocation) = admission("cwp://fixture.test/invocations/interrupted");
        let admitted = receipt(
            coordinator
                .admit(context(), &record, &invocation, "admit".into(), 10)
                .unwrap(),
        );
        let assigned = receipt(
            coordinator
                .assign(
                    context(),
                    record.invocation_ref.clone(),
                    admitted.revision,
                    Assignment {
                        executor_ref: "cwp://fixture.test/executors/a".into(),
                        assignment_ref: "cwp://fixture.test/assignments/a".into(),
                        generation: 1,
                        route_fingerprint: "route".into(),
                        expires_at_unix_ms: 100,
                    },
                    "assign".into(),
                    10,
                )
                .unwrap(),
        );
        let token = coordinator
            .assignment_token(&context(), &record.invocation_ref)
            .unwrap();
        let stopped = receipt(
            coordinator
                .stop(
                    context(),
                    record.invocation_ref.clone(),
                    assigned.revision,
                    "interrupt".into(),
                    CloudInterruption {
                        cancelled: false,
                        uncertain_effects: json!(["provider acknowledgement lost"]),
                    },
                    101,
                )
                .unwrap(),
        );
        assert_eq!(stopped.terminal, Some(EventKind::Interrupted));
        let frame = json!({"type":"result","invocation_ref":record.invocation_ref,"sequence":1,"status":200,"summary":"Late.","result":{}});
        assert!(coordinator
            .publish(
                context(),
                record.invocation_ref.clone(),
                &token,
                stopped.revision,
                CloudCallback {
                    request_id: "late".into(),
                    sequence: 1,
                    frame
                },
                102
            )
            .is_err());
        let events = coordinator
            .storage
            .events(
                &context(),
                &record.invocation_ref,
                None,
                10,
                &CurrentAuthority,
            )
            .unwrap();
        assert_eq!(events.items[0].payload["replay_permitted"], false);
        assert_eq!(
            events.items[0].payload["uncertain_effects"],
            json!(["provider acknowledgement lost"])
        );
    }
    include!("recovery_tests.rs");
}
