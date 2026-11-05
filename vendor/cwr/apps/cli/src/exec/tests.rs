#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_workspace() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        )
    }

    #[test]
    fn intrinsic_stop_uses_the_resumed_conversation() {
        let temporary = tempfile::tempdir().expect("temporary CLI state");
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            cwr::v2026_09::local::LocalHostIdentity::new(
                "~@/actors/person",
                "cwp://org.coworkerprotocol.local/integrations/cli",
            ),
        )
        .expect("open local runtime");
        let admission = runtime
            .admit_conversation_text(None, "@helper coordinate")
            .expect("admit helper work");
        assert_eq!(
            admission.record.integration_ref,
            "cwp://org.coworkerprotocol.local/integrations/cli"
        );
        let command = cwr::intrinsic_cwr::parse_intrinsic_cwr_command("@cwr stop")
            .expect("parse intrinsic stop");

        let response =
            execute_intrinsic_command(&runtime, &command, Some(admission.conversation_ref()))
                .expect("execute intrinsic stop");
        assert!(matches!(
            response,
            IntrinsicCwrResponse::InvocationStopped { .. }
        ));
    }

    #[test]
    fn interruption_targets_the_invocation_that_started() {
        let mut execution = LocalExecutionTracker::default();
        assert_eq!(execution.interruption_target(), None);

        execution.observe(&LocalRuntimeEvent::InvocationStarted {
            invocation_ref: "recovered".into(),
            target_actor_ref: "~@/actors/helper".into(),
        });
        assert_eq!(execution.interruption_target(), Some("recovered"));

        execution.mark_stop_attempted();
        assert_eq!(execution.interruption_target(), None);
        execution.observe(&LocalRuntimeEvent::InvocationStarted {
            invocation_ref: "current".into(),
            target_actor_ref: "~@/actors/helper".into(),
        });
        assert_eq!(execution.interruption_target(), Some("current"));
    }

    #[test]
    fn non_success_terminal_lines_are_status_not_coworker_speech() {
        let blocked = ActionResponse {
            progress: None,
            invocation_ref: "blocked".into(),
            sequence: 1,
            status: cwr::v2026_09::invocation::ActionResponseStatus::new(451).unwrap(),
            summary: "Approval is required.".into(),
            result: None,
            error: Some(
                ActionResponseDetail::new("approval_required")
                    .with_message("Ask a workspace owner.")
                    .with_details(serde_json::json!({"scope": "publish"})),
            ),
            cause: None,
        };
        assert_eq!(
            terminal_status_lines("@helper", &blocked),
            vec![
                "@helper blocked (451): Approval is required.",
                "@helper error: approval_required — Ask a workspace owner.",
            ]
        );

        let stopped = ActionResponse {
            progress: None,
            invocation_ref: "stopped".into(),
            sequence: 1,
            status: cwr::v2026_09::invocation::ActionResponseStatus::STOPPED,
            summary: "Work was interrupted.".into(),
            result: None,
            error: None,
            cause: Some(
                ActionResponseDetail::new("operator_stop")
                    .with_message("Stopped from the terminal."),
            ),
        };
        let lines = terminal_status_lines("@helper", &stopped);
        assert_eq!(
            lines,
            vec![
                "@helper stopped (499): Work was interrupted.",
                "@helper cause: operator_stop — Stopped from the terminal.",
            ]
        );
        assert!(lines.iter().all(|line| !line.starts_with("@helper:")));
    }
}
