fn restart_schedule_fixture(temporary: &tempfile::TempDir, runtime: LocalRuntime) -> LocalRuntime {
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
    let actor = add_participation_actor(&runtime, "schedule_fixture");
    mark_actor_ready_for_test(&runtime, &actor);
    policy.suspended = false;
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    runtime
}

#[test]
fn schedules_spike_archive_interruption_recovers_after_restart_without_execution() {
    for fault in [ScheduleFault::BeforeArchive, ScheduleFault::AfterArchive] {
        let (temporary, runtime, _, a, _) = installed_schedule_fixture();
        runtime.tick_schedules(2000).unwrap();
        let first = runtime.tick_schedules(2001).unwrap().remove(0);
        runtime
            .stop_invocation(first.invocation_ref().unwrap(), None)
            .unwrap();
        assert!(runtime.tick_schedules_with_fault(2002, fault).is_err());
        let person = participation_ref(&runtime, "person");
        let service = runtime.schedules_module_as(&a, &person).unwrap();
        let schedule = service.list().unwrap().pop().unwrap();
        let before = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
        let run = before
            .iter()
            .find(|r| r.invocation_ref.as_deref() == first.invocation_ref())
            .unwrap();
        assert!(run.outcome.starts_with("terminal_"));
        assert!(!run.archived);
        assert_eq!(
            runtime
                .conversation(first.conversation_ref())
                .unwrap()
                .unwrap()
                .archived,
            fault == ScheduleFault::AfterArchive
        );
        let count = runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count();
        let runtime = restart_schedule_fixture(&temporary, runtime);
        runtime.tick_schedules(2003).unwrap();
        runtime.tick_schedules(2004).unwrap();
        let service = runtime.schedules_module_as(&a, &person).unwrap();
        let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
        let recovered = runs
            .iter()
            .find(|r| r.occurrence_ref == run.occurrence_ref)
            .unwrap();
        assert_eq!(recovered.outcome, run.outcome);
        assert_eq!(recovered.invocation_ref, run.invocation_ref);
        assert!(recovered.archived);
        assert!(
            runtime
                .conversation(first.conversation_ref())
                .unwrap()
                .unwrap()
                .archived
        );
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
}

#[test]
fn archived_schedule_history_is_not_scanned_on_quiet_timer_ticks() {
    let (_temporary, runtime, _, a, b) = installed_schedule_fixture();
    for due in (2000..=20_000).step_by(2000) {
        runtime.tick_schedules(due).unwrap();
        for admission in runtime.tick_schedules(due + 1).unwrap() {
            runtime
                .stop_invocation(admission.invocation_ref().unwrap(), None)
                .unwrap();
        }
        runtime.tick_schedules(due + 2).unwrap();
    }
    let person = participation_ref(&runtime, "person");
    for owner in [&a, &b] {
        let service = runtime.schedules_module_as(owner, &person).unwrap();
        let schedule = service.list().unwrap().pop().unwrap();
        service.set_paused(&schedule.schedule_ref, true, 20_003).unwrap();
        assert!(service
            .recent_runs(&schedule.schedule_ref, 20)
            .unwrap()
            .iter()
            .any(|run| run.archived));
    }
    let baseline = runtime.lock().unwrap().schedule_archive_history_scan_count;
    assert_eq!(baseline, 1, "only the boot recovery sweep should have run");
    for now in (20_100..30_000).step_by(100) {
        runtime.tick_schedules(now).unwrap();
    }
    assert_eq!(runtime.lock().unwrap().schedule_archive_history_scan_count, baseline);
    runtime.tick_schedules(30_000).unwrap();
    assert_eq!(runtime.lock().unwrap().schedule_archive_history_scan_count, baseline + 1);
}

#[test]
fn schedules_spike_disable_fences_pending_work_and_recovers_committed_receipt() {
    for availability in ["disabled", "needs_setup"] {
        for committed in [false, true] {
            let (_temporary, runtime, definition, a, _) = installed_schedule_fixture();
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
            runtime
                .configure_installed_schedules_as(&person, &a, availability, vec![], 2002)
                .unwrap();
            runtime.tick_schedules(2003).unwrap();
            let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
            assert_eq!(
                runs.iter().filter(|r| r.invocation_ref.is_some()).count(),
                usize::from(committed)
            );
            if !committed {
                assert!(runs.iter().any(|r| r.outcome == "skipped_unavailable"));
            }
            let count = runtime
                .lock()
                .unwrap()
                .admission
                .invocation_records()
                .count();
            runtime.tick_schedules(6000).unwrap();
            assert_eq!(
                runtime
                    .lock()
                    .unwrap()
                    .admission
                    .invocation_records()
                    .count(),
                count
            );
            runtime
                .configure_installed_schedules_as(
                    &person,
                    &a,
                    "ready",
                    vec![schedule_activation(&definition)],
                    6001,
                )
                .unwrap();
            if committed {
                runtime
                    .stop_invocation(
                        runs.iter()
                            .find_map(|r| r.invocation_ref.as_deref())
                            .unwrap(),
                        None,
                    )
                    .unwrap();
            }
            runtime.tick_schedules(6002).unwrap();
            assert_eq!(
                service
                    .get(&schedule.schedule_ref)
                    .unwrap()
                    .next_due_unix_ms,
                8000
            );
            runtime.tick_schedules(8000).unwrap();
            runtime.tick_schedules(8001).unwrap();
            let restored = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
            assert!(
                restored
                    .iter()
                    .any(|r| r.due_unix_ms == 8000 && r.invocation_ref.is_some())
            );
            assert!(
                !restored
                    .iter()
                    .any(|r| r.due_unix_ms == 4000 || r.due_unix_ms == 6000)
            );
        }
    }
}

#[test]
fn schedules_spike_offline_restart_skips_elapsed_pending_and_idle_ticks() {
    for reserved in [false, true] {
        let (temporary, runtime, _, a, _) = installed_schedule_fixture();
        let person = participation_ref(&runtime, "person");
        let schedule = runtime
            .schedules_module_as(&a, &person)
            .unwrap()
            .list()
            .unwrap()
            .pop()
            .unwrap();
        if reserved {
            assert!(
                runtime
                    .tick_schedules_with_fault(2000, ScheduleFault::AfterReservation)
                    .is_err()
            );
        }
        let runtime = restart_schedule_fixture(&temporary, runtime);
        runtime.tick_schedules(10000).unwrap();
        runtime.tick_schedules(10001).unwrap();
        let service = runtime.schedules_module_as(&a, &person).unwrap();
        let saved = service.get(&schedule.schedule_ref).unwrap();
        assert_eq!(saved.next_due_unix_ms, 12000);
        assert!(saved.active_occurrence_ref.is_none());
        let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
        assert!(runs.iter().all(|r| r.invocation_ref.is_none()));
        if reserved {
            assert!(runs.iter().any(|r| r.outcome == "skipped_offline"));
        }
    }
}

#[test]
fn schedules_spike_worker_fences_target_and_authority_loss_then_uses_future_tick() {
    for remove_route in [false, true] {
        for committed in [false, true] {
            let (_temporary, runtime, declaration, a, _) = installed_schedule_fixture();
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
            let original_routes = runtime.lock().unwrap().graph.routes.clone();
            let original_policy = runtime
                .lock()
                .unwrap()
                .admission
                .context_policy
                .clone()
                .unwrap();
            if remove_route {
                runtime
                    .lock()
                    .unwrap()
                    .graph
                    .routes
                    .retain(|r| r.actor_ref != declaration.target.coworker_ref);
            } else {
                let mut policy = original_policy.clone();
                policy.contexts.insert(a.clone(), vec![]);
                runtime
                    .configure_conversation_contexts(Some(policy))
                    .unwrap();
            }
            runtime.tick_schedules(2003).unwrap();
            let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
            let old = runs.iter().find(|r| r.due_unix_ms == 2000).unwrap();
            assert_eq!(old.invocation_ref.is_some(), committed);
            if !committed {
                assert_eq!(old.outcome, "admission_failed");
                assert!(
                    service
                        .get(&schedule.schedule_ref)
                        .unwrap()
                        .last_admission_error
                        .is_some()
                );
            }
            runtime.lock().unwrap().graph.routes = original_routes;
            runtime
                .configure_conversation_contexts(Some(original_policy))
                .unwrap();
            if committed {
                runtime
                    .stop_invocation(old.invocation_ref.as_deref().unwrap(), None)
                    .unwrap();
                runtime.tick_schedules(2004).unwrap();
            }
            runtime.tick_schedules(4000).unwrap();
            runtime.tick_schedules(4001).unwrap();
            let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
            let new = runs.iter().find(|r| r.due_unix_ms == 4000).unwrap();
            assert!(new.invocation_ref.is_some());
            assert!(
                service
                    .get(&schedule.schedule_ref)
                    .unwrap()
                    .last_admission_error
                    .is_none()
            );
            assert_ne!(new.occurrence_ref, old.occurrence_ref);
            assert_eq!(runs.iter().filter(|r| r.due_unix_ms == 2000).count(), 1);
        }
    }
}

#[test]
fn schedules_spike_active_update_preserves_occurrence_revision_and_changes_future_tick() {
    let (_temporary, runtime, mut declaration, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let schedule = service.list().unwrap().pop().unwrap();
    assert!(
        runtime
            .tick_schedules_with_fault(2000, ScheduleFault::AfterReservation)
            .is_err()
    );
    let original = declaration.clone();
    declaration.revision = "v2".into();
    declaration.interval_ms = 3000;
    let updated = service.install(declaration.clone(), 2001).unwrap();
    assert_eq!(updated.schedule_ref, schedule.schedule_ref);
    runtime.tick_schedules(2002).unwrap();
    let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
    let active = runs.iter().find(|r| r.invocation_ref.is_some()).unwrap();
    assert_eq!(active.definition, original);
    runtime
        .stop_invocation(active.invocation_ref.as_deref().unwrap(), None)
        .unwrap();
    runtime.tick_schedules(2003).unwrap();
    assert_eq!(
        service
            .get(&schedule.schedule_ref)
            .unwrap()
            .next_due_unix_ms,
        3000
    );
    runtime.tick_schedules(3000).unwrap();
    runtime.tick_schedules(3001).unwrap();
    let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
    let next = runs.iter().find(|r| r.due_unix_ms == 3000).unwrap();
    assert_eq!(next.definition, declaration);
    assert!(next.invocation_ref.is_some());
}

#[test]
fn schedules_spike_pending_archive_retry_does_not_block_the_next_execution() {
    let (_temporary, runtime, _, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let service = runtime.schedules_module_as(&a, &person).unwrap();
    let schedule = service.list().unwrap().pop().unwrap();
    runtime.tick_schedules(2000).unwrap();
    runtime.tick_schedules(2001).unwrap();
    let old = service
        .recent_runs(&schedule.schedule_ref, 10)
        .unwrap()
        .pop()
        .unwrap();
    runtime
        .stop_invocation(old.invocation_ref.as_deref().unwrap(), None)
        .unwrap();
    for now in [2002, 4000, 4001] {
        assert!(
            runtime
                .tick_schedules_with_fault(now, ScheduleFault::BeforeArchive)
                .is_err()
        );
    }
    let runs = service.recent_runs(&schedule.schedule_ref, 10).unwrap();
    let pending_archive = runs
        .iter()
        .find(|r| r.occurrence_ref == old.occurrence_ref)
        .unwrap();
    assert!(!pending_archive.archived);
    assert!(pending_archive.outcome.starts_with("terminal_"));
    let new = runs.iter().find(|r| r.due_unix_ms == 4000).unwrap();
    assert!(new.invocation_ref.is_some());
    assert_ne!(new.invocation_ref, old.invocation_ref);
    let updates = runtime.tick_schedules_with_updates(4002).unwrap();
    assert!(
        updates
            .archived_conversation_refs
            .contains(old.conversation_ref.as_ref().unwrap())
    );
    assert!(
        service
            .recent_runs(&schedule.schedule_ref, 10)
            .unwrap()
            .iter()
            .find(|r| r.occurrence_ref == old.occurrence_ref)
            .unwrap()
            .archived
    );
}
