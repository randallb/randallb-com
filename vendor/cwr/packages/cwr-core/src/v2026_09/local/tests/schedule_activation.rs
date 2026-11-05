fn schedule_activation(
    definition: &ScheduleDeclaration,
) -> crate::v2026_09::upstream::InstalledScheduleActivation {
    crate::v2026_09::upstream::InstalledScheduleActivation {
        declaration_ref: definition.declaration_ref.clone(),
        revision: definition.revision.clone(),
        target_actor_ref: definition.target.coworker_ref.clone(),
        action_name: definition.target.action_name.clone(),
        input: definition.input.clone(),
        anchor_unix_ms: definition.anchor_unix_ms,
        interval_ms: definition.interval_ms,
    }
}

#[test]
fn schedules_spike_activation_fences_removed_declarations_and_preserves_author() {
    let (_temporary, runtime, mut definition, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let viewer = "cwp://org.example/people/another-viewer".to_owned();
    {
        let mut state = runtime.lock().unwrap();
        let mut actor = state
            .graph
            .actors
            .iter()
            .find(|a| a.actor_ref == person)
            .unwrap()
            .clone();
        actor.actor_ref = viewer.clone();
        actor.coworker_ref = viewer.clone();
        actor.coworker_slug = "another_viewer".into();
        state.graph.actors.push(actor);
        state.catalog = local_catalog(&state.graph, &state.host_identity.acting_actor_ref).unwrap();
    }
    let mut policy = runtime
        .lock()
        .unwrap()
        .admission
        .context_policy
        .clone()
        .unwrap();
    policy.contexts.get_mut(&a).unwrap().push(viewer.clone());
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let original = service.list().unwrap().pop().unwrap();
    runtime
        .configure_installed_schedules_as(
            &viewer,
            &a,
            "ready",
            vec![schedule_activation(&definition)],
            1,
        )
        .unwrap();
    assert_eq!(
        service
            .get(&original.schedule_ref)
            .unwrap()
            .author_actor_ref,
        person
    );
    assert!(
        runtime
            .tick_schedules_with_fault(2000, ScheduleFault::AfterReservation)
            .is_err()
    );
    runtime
        .configure_installed_schedules_as(&viewer, &a, "ready", vec![], 2001)
        .unwrap();
    runtime.tick_schedules(2002).unwrap();
    let removed = service.get(&original.schedule_ref).unwrap();
    assert_eq!(removed.availability, "removed");
    let runs = service.recent_runs(&original.schedule_ref, 10).unwrap();
    assert!(runs.iter().all(|r| r.invocation_ref.is_none()));
    assert!(runs.iter().any(|r| r.outcome == "skipped_unavailable"));
    definition.declaration_ref.push_str("-renamed");
    runtime
        .configure_installed_schedules_as(
            &viewer,
            &a,
            "ready",
            vec![schedule_activation(&definition)],
            2003,
        )
        .unwrap();
    let schedules = service.list().unwrap();
    assert_eq!(schedules.len(), 2);
    assert_eq!(
        service.get(&original.schedule_ref).unwrap().availability,
        "removed"
    );
    let renamed = schedules
        .iter()
        .find(|s| s.schedule_ref != original.schedule_ref)
        .unwrap();
    assert_eq!(renamed.author_actor_ref, viewer);
    runtime.tick_schedules(4000).unwrap();
    runtime.tick_schedules(4001).unwrap();
    assert!(
        service
            .recent_runs(&renamed.schedule_ref, 10)
            .unwrap()
            .iter()
            .any(|r| r.invocation_ref.is_some())
    );
    assert!(
        service
            .recent_runs(&original.schedule_ref, 10)
            .unwrap()
            .iter()
            .all(|r| r.invocation_ref.is_none())
    );
}

#[test]
fn schedules_spike_rejects_intervals_shorter_than_supported_worker_cadence() {
    let (_temporary, runtime, mut definition, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    for interval in [0, 1, 100, 999] {
        definition.interval_ms = interval;
        assert!(service.install(definition.clone(), 1).is_err());
    }
    definition.interval_ms = 1000;
    definition.revision = "minimum-supported".into();
    let installed = service.install(definition, 1).unwrap();
    assert_eq!(installed.next_due_unix_ms, 1000);
    runtime.tick_schedules(1000).unwrap();
    runtime.tick_schedules(1100).unwrap();
    assert!(
        service
            .recent_runs(&installed.schedule_ref, 10)
            .unwrap()
            .iter()
            .any(|r| r.invocation_ref.is_some())
    );
}
