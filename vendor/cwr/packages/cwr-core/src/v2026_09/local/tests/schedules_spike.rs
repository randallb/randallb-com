fn installed_schedule_fixture() -> (
    tempfile::TempDir,
    LocalRuntime,
    ScheduleDeclaration,
    String,
    String,
) {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let actor = add_participation_actor(&runtime, "schedule_fixture");
    mark_actor_ready_for_test(&runtime, &actor);
    let a = "urn:uuid:11111111-1111-4111-8111-111111111111".to_string();
    let b = "urn:uuid:22222222-2222-4222-8222-222222222222".to_string();
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: vec![],
            suspended: false,
            default_context_ref: "cwp://example.test/workspace".into(),
            contexts: BTreeMap::from([
                ("cwp://example.test/workspace".into(), vec![]),
                (a.clone(), vec![actor.clone()]),
                (b.clone(), vec![actor.clone()]),
            ]),
        }))
        .unwrap();
    let route = runtime
        .graph()
        .unwrap()
        .routes
        .into_iter()
        .find(|r| r.actor_ref == actor && r.action_name == "draft--brief")
        .unwrap();
    let declaration = ScheduleDeclaration {
        declaration_ref: "cwp://example.test/releases/fictional/schedules/tick".into(),
        revision: "v1".into(),
        target: route_action_target(&runtime.graph().unwrap().runtime_ref, &route),
        input: json!({"private_token": "CWR_SCHEDULE_PRIVATE_SENTINEL"}),
        anchor_unix_ms: 0,
        interval_ms: 2000,
    };
    let service_a = runtime.schedules_module_as(&a, &person).unwrap();
    let installed_a = service_a.install(declaration.clone(), 0).unwrap();
    let installed_b = runtime
        .schedules_module_as(&b, &person)
        .unwrap()
        .install(declaration.clone(), 0)
        .unwrap();
    assert_ne!(installed_a.schedule_ref, installed_b.schedule_ref);
    assert_eq!(
        service_a.install(declaration.clone(), 0).unwrap(),
        installed_a
    );
    runtime.tick_schedules(0).unwrap();
    (temporary, runtime, declaration, a, b)
}

#[test]
fn schedules_spike_isolates_controls_and_preserves_pause_and_update_identity() {
    let (temporary, runtime, mut declaration, a, b) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let service_a = runtime.schedules_module_as(&a, &person).unwrap();
    let service_b = runtime.schedules_module_as(&b, &person).unwrap();
    let sa = service_a.list().unwrap().pop().unwrap();
    let sb = service_b.list().unwrap().pop().unwrap();
    assert!(service_a.get(&sb.schedule_ref).is_err());
    assert!(service_a.set_paused(&sb.schedule_ref, true, 0).is_err());
    assert!(service_a.set_paused(&sb.schedule_ref, false, 0).is_err());
    assert!(service_a.recent_runs(&sb.schedule_ref, 10).is_err());
    service_a.set_paused(&sa.schedule_ref, true, 0).unwrap();
    assert!(!service_b.get(&sb.schedule_ref).unwrap().paused);
    declaration.revision = "v2".into();
    declaration.interval_ms = 3000;
    let updated = service_a.install(declaration, 1000).unwrap();
    assert_eq!(updated.schedule_ref, sa.schedule_ref);
    assert!(updated.paused);
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    assert!(service.get(&sa.schedule_ref).unwrap().paused);
    let resumed = service.set_paused(&sa.schedule_ref, false, 10000).unwrap();
    assert_eq!(resumed.next_due_unix_ms, 12000);
}

#[test]
fn schedules_spike_reserves_recovers_links_and_skips_overlap() {
    let (_temporary, runtime, _declaration, a, b) = installed_schedule_fixture();
    assert!(
        runtime
            .tick_schedules_with_fault(2000, ScheduleFault::AfterReservation)
            .is_err()
    );
    assert!(
        runtime
            .tick_schedules_with_fault(2001, ScheduleFault::AfterAdmission)
            .is_err()
    );
    let before = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .count();
    assert!(
        runtime
            .tick_schedules_with_fault(2002, ScheduleFault::AfterLink)
            .is_err()
    );
    runtime.tick_schedules(2003).unwrap();
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count(),
        before
    );
    runtime.tick_schedules(4000).unwrap();
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let schedule = service.list().unwrap().pop().unwrap();
    let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
    assert!(runs.iter().any(|r| r.outcome == "skipped_overlap"));
    let first = runs.iter().find(|r| r.invocation_ref.is_some()).unwrap();
    let conversation = runtime
        .conversation(first.conversation_ref.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(conversation.owner_context_ref.as_deref(), Some(a.as_str()));
    assert!(conversation.is_sidebar_eligible());
    runtime.tick_schedules(6000).unwrap();
    runtime.tick_schedules(6001).unwrap();
    let service_b = runtime.schedules_module_as(&b, &person).unwrap();
    let sb = service_b.list().unwrap().pop().unwrap();
    let runs_b = service_b.recent_runs(&sb.schedule_ref, 10).unwrap();
    let run_b = runs_b.iter().find(|r| r.invocation_ref.is_some()).unwrap();
    assert_eq!(
        runtime
            .conversation(run_b.conversation_ref.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .owner_context_ref
            .as_deref(),
        Some(b.as_str())
    );
}

#[test]
fn schedules_spike_archives_terminal_runs_without_repeating_execution() {
    let (_temporary, runtime, _declaration, a, _) = installed_schedule_fixture();
    runtime.tick_schedules(2000).unwrap();
    let admitted = runtime.tick_schedules(2001).unwrap();
    let first = &admitted[0];
    runtime
        .stop_invocation(first.invocation_ref().unwrap(), None)
        .unwrap();
    let updates = runtime.tick_schedules_with_updates(2002).unwrap();
    assert!(updates.admissions.is_empty());
    assert!(
        updates
            .archived_conversation_refs
            .contains(&first.conversation_ref().to_owned())
    );
    let conversation = runtime
        .conversation(first.conversation_ref())
        .unwrap()
        .unwrap();
    assert!(conversation.archived);
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let schedule = service.list().unwrap().pop().unwrap();
    let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
    assert!(
        runs.iter()
            .any(|r| r.archived && r.outcome.starts_with("terminal_"))
    );
    let count = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .count();
    runtime.tick_schedules(2003).unwrap();
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count(),
        count
    );
}

#[test]
fn schedules_spike_disable_reenable_keeps_explicit_pause_and_history() {
    let (_temporary, runtime, definition, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let schedule = service.list().unwrap().pop().unwrap();
    service.set_paused(&schedule.schedule_ref, true, 1).unwrap();
    runtime
        .configure_installed_schedules_as(&person, &a, "disabled", vec![], 2)
        .unwrap();
    runtime
        .configure_installed_schedules_as(
            &person,
            &a,
            "ready",
            vec![schedule_activation(&definition)],
            3000,
        )
        .unwrap();
    let saved = service.get(&schedule.schedule_ref).unwrap();
    assert!(saved.paused);
    assert_eq!(saved.availability, "ready");
    runtime.tick_schedules(4000).unwrap();
    runtime.tick_schedules(4001).unwrap();
    assert!(
        service
            .recent_runs(&schedule.schedule_ref, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn schedules_spike_restart_recovers_each_fault_window_without_duplicate_invocations() {
    for fault in [
        ScheduleFault::AfterReservation,
        ScheduleFault::AfterAdmission,
        ScheduleFault::AfterLink,
    ] {
        let (temporary, runtime, declaration, a, _) = installed_schedule_fixture();
        let person = participation_ref(&runtime, "person");
        if fault == ScheduleFault::AfterReservation {
            assert!(runtime.tick_schedules_with_fault(2000, fault).is_err());
        } else {
            runtime.tick_schedules(2000).unwrap();
            assert!(runtime.tick_schedules_with_fault(2001, fault).is_err());
        }
        let mut policy = runtime
            .lock()
            .unwrap()
            .admission
            .context_policy
            .clone()
            .unwrap();
        drop(runtime);
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("runtime.sqlite"),
            fixture_host_identity(),
        )
        .unwrap();
        let target = add_participation_actor(&runtime, "schedule_fixture");
        mark_actor_ready_for_test(&runtime, &target);
        assert_eq!(target, declaration.target.coworker_ref);
        policy.suspended = false;
        runtime
            .configure_conversation_contexts(Some(policy))
            .unwrap();
        runtime.tick_schedules(2002).unwrap();
        runtime.tick_schedules(2003).unwrap();
        let service = runtime.schedules_module_as(&a, &person).unwrap();
        let schedule = service.list().unwrap().pop().unwrap();
        let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
        let run = runs.iter().find(|r| r.invocation_ref.is_some()).unwrap();
        let count = runtime.lock().unwrap().admission.invocation_records().filter(|record|
            matches!(&record.origin, InvocationOrigin::Ingress { integration_ref, provider_event_ref }
                if integration_ref == SCHEDULE_INGRESS && provider_event_ref == &run.occurrence_ref)
        ).count();
        assert_eq!(count, 1, "{fault:?}");
        assert_eq!(run.definition, declaration);
        runtime.tick_schedules(2004).unwrap();
        assert_eq!(
            service.recent_runs(&schedule.schedule_ref, 10).unwrap(),
            runs
        );
    }
}

#[test]
fn schedules_spike_pause_fences_reserved_work_but_preserves_a_committed_receipt() {
    for committed in [false, true] {
        let (_temporary, runtime, _, a, _) = installed_schedule_fixture();
        let person = participation_ref(&runtime, "person");
        let service = runtime.schedules_module_as(&a, &person).unwrap();
        let schedule = service.list().unwrap().pop().unwrap();
        assert!(
            runtime
                .tick_schedules_with_fault(2000, ScheduleFault::AfterReservation)
                .is_err()
        );
        if committed {
            assert!(
                runtime
                    .tick_schedules_with_fault(2001, ScheduleFault::AfterAdmission)
                    .is_err()
            );
        }
        service
            .set_paused(&schedule.schedule_ref, true, 2002)
            .unwrap();
        runtime.tick_schedules(2003).unwrap();
        let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
        assert_eq!(
            runs.iter().filter(|r| r.invocation_ref.is_some()).count(),
            usize::from(committed)
        );
        assert!(service.get(&schedule.schedule_ref).unwrap().paused);
    }
}

include!("schedule_control_actions.rs");

include!("schedule_occurrence_reads.rs");

include!("schedule_recovery.rs");

include!("schedule_activation.rs");
