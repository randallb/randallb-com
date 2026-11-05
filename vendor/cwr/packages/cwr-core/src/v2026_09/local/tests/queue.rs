mod execution_queue {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn copy_workspace(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".cwr" {
                continue;
            }
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_workspace(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn runtime(workspace: &Path, host: &str) -> LocalRuntime {
        runtime_with_options(workspace, host, LocalRuntimeOptions::default())
    }

    fn runtime_with_options(
        workspace: &Path,
        host: &str,
        options: LocalRuntimeOptions,
    ) -> LocalRuntime {
        let runtime = LocalRuntime::open_with_state_path_and_options(
            workspace.join("CWR.toml"),
            workspace.join(format!(".cwr/{host}.json")),
            LocalHostIdentity::new(load_default_local_acting_actor_ref(workspace.join("CWR.toml")).unwrap(), fixture_host_identity().ingress_integration_ref),
            options,
        )
        .unwrap();
        // Exercise the real claim and failure settlement without launching a
        // provider. Observers hold an attempt at its execution boundary.
        for binding in &mut runtime.lock().unwrap().graph.actor_integrations {
            binding.adapter_ref = "cwp://example.test/adapters/unavailable".into();
        }
        runtime
    }

    fn observe_starts<'a>(
        started: &'a mpsc::Sender<String>,
        released: Option<&'a mpsc::Receiver<()>>,
    ) -> impl FnMut(LocalRuntimeEvent) + 'a {
        move |event| {
            let LocalRuntimeEvent::InvocationStarted { invocation_ref, .. } = event else {
                return;
            };
            started.send(invocation_ref).unwrap();
            if let Some(released) = released {
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
    }

    fn observe_start_and_wait(
        started: mpsc::Sender<String>,
        released: mpsc::Receiver<()>,
    ) -> impl FnMut(LocalRuntimeEvent) {
        move |event| {
            let LocalRuntimeEvent::InvocationStarted { invocation_ref, .. } = event else {
                return;
            };
            started.send(invocation_ref).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }

    fn observe_selected_start_and_wait(
        started: mpsc::Sender<String>,
        released: mpsc::Receiver<()>,
        blocked_refs: BTreeSet<String>,
    ) -> impl FnMut(LocalRuntimeEvent) {
        move |event| {
            let LocalRuntimeEvent::InvocationStarted { invocation_ref, .. } = event else {
                return;
            };
            started.send(invocation_ref.clone()).unwrap();
            if blocked_refs.contains(&invocation_ref) {
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
    }

    fn assert_pending(runtime: &LocalRuntime, admission: &LocalAdmission) {
        let turn = runtime.turn(&admission.record).unwrap();
        let record = turn.invocation.unwrap();
        assert_eq!(record.state, ActionInvocationState::Admitted);
        assert_eq!(record.attempt_count, 0);
        assert!(record.responses.is_empty());
    }

    #[test]
    fn synchronous_execution_leaves_unrelated_queued_work_for_the_outer_loop() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let runtime = runtime(temporary.path(), "local");
        let unrelated = runtime.admit_text(None, "@helper unrelated").unwrap();
        let target = runtime.admit_text(None, "@helper target").unwrap();
        let unrelated_ref = unrelated.invocation_ref().unwrap().to_owned();
        let target_ref = target.invocation_ref().unwrap().to_owned();
        let mut queue = VecDeque::from([unrelated_ref.clone(), target_ref.clone()]);

        let state = runtime.lock().unwrap();
        assert_eq!(
            take_next_synchronous_invocation(&state.admission, &mut queue, &target_ref),
            Some(target_ref)
        );
        assert_eq!(queue, VecDeque::from([unrelated_ref]));
    }

    #[test]
    fn different_conversations_overlap_while_same_conversation_turns_wait() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let runtime = runtime(temporary.path(), "local");
        let first = runtime.admit_text(None, "@helper first").unwrap();
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        thread::scope(|scope| {
            let first_runtime = runtime.clone();
            let first_admission = first.clone();
            let first_worker = scope.spawn(move || {
                first_runtime
                    .execute_admission(&first_admission, observe_starts(&started, Some(&released)))
                    .unwrap()
            });
            assert_eq!(
                starts.recv_timeout(Duration::from_secs(2)).unwrap(),
                first.invocation_ref().unwrap()
            );
            let second = runtime
                .admit_text(Some(first.conversation_ref()), "@helper second")
                .unwrap();
            let third = runtime.admit_text(None, "@helper third").unwrap();
            let cancelled = runtime
                .admit_text(Some(first.conversation_ref()), "@helper cancel me")
                .unwrap();
            let (second_started, second_starts) = mpsc::channel();
            let second_runtime = runtime.clone();
            let second_admission = second.clone();
            let second_worker = scope.spawn(move || {
                second_runtime
                    .execute_admission(&second_admission, observe_starts(&second_started, None))
                    .unwrap()
            });
            let (third_started, third_starts) = mpsc::channel();
            let third_runtime = runtime.clone();
            let third_admission = third.clone();
            let third_worker = scope.spawn(move || {
                third_runtime
                    .execute_admission(&third_admission, observe_starts(&third_started, None))
                    .unwrap()
            });
            assert_pending(&runtime, &second);
            assert!(matches!(
                runtime
                    .stop_intrinsic_invocation(
                        Some(first.conversation_ref()),
                        cancelled.invocation_ref(),
                        None,
                    )
                    .unwrap(),
                IntrinsicCwrResponse::InvocationStopped { .. }
            ));
            assert_eq!(
                runtime
                    .turn(&first.record)
                    .unwrap()
                    .invocation
                    .unwrap()
                    .state,
                ActionInvocationState::Running
            );
            assert!(second_starts
                .recv_timeout(Duration::from_millis(80))
                .is_err());
            assert_eq!(
                third_starts.recv_timeout(Duration::from_secs(2)).unwrap(),
                third.invocation_ref().unwrap()
            );
            assert_eq!(
                third_worker.join().unwrap().invocation.unwrap().state,
                ActionInvocationState::Failed
            );
            release.send(()).unwrap();
            assert_eq!(
                first_worker.join().unwrap().invocation.unwrap().state,
                ActionInvocationState::Failed
            );
            assert_eq!(
                second_starts.recv_timeout(Duration::from_secs(2)).unwrap(),
                second.invocation_ref().unwrap()
            );
            assert_eq!(
                second_worker.join().unwrap().invocation.unwrap().state,
                ActionInvocationState::Failed
            );
            let stopped = runtime.turn(&cancelled.record).unwrap().invocation.unwrap();
            assert_eq!(stopped.state, ActionInvocationState::Stopped);
            assert_eq!(stopped.attempt_count, 0);
            assert_eq!(stopped.responses.last().unwrap().status.as_u16(), 499);
            assert!(runtime.conversation_turn_statuses().unwrap().is_empty());
        });
    }

    #[test]
    fn separate_hosts_execute_concurrently_in_one_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("shared");
        let independent = temporary.path().join("independent");
        copy_workspace(fixture_workspace().parent().unwrap(), &workspace);
        copy_workspace(fixture_workspace().parent().unwrap(), &independent);
        let desktop = runtime(&workspace, "desktop");
        let slack = runtime(&workspace, "slack");
        let other = runtime(&independent, "other");
        let first = desktop.admit_text(None, "@helper desktop").unwrap();
        let concurrent = slack.admit_text(None, "@helper slack").unwrap();
        let unrelated = other.admit_text(None, "@helper unrelated").unwrap();
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        thread::scope(|scope| {
            let desktop_worker = scope.spawn(move || {
                desktop
                    .execute_admission(&first, observe_starts(&started, Some(&released)))
                    .unwrap()
            });
            starts.recv_timeout(Duration::from_secs(2)).unwrap();
            let (finished, finishes) = mpsc::channel();
            let concurrent_runtime = slack.clone();
            let concurrent_admission = concurrent.clone();
            let slack_worker = scope.spawn(move || {
                let turn = concurrent_runtime
                    .execute_admission(&concurrent_admission, |_| {})
                    .unwrap();
                finished.send(turn).unwrap();
            });
            let finished = finishes
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .invocation
                .unwrap();
            assert_eq!(finished.state, ActionInvocationState::Failed);
            assert_eq!(finished.attempt_count, 1);
            slack_worker.join().unwrap();
            let unrelated_turn = other.execute_admission(&unrelated, |_| {}).unwrap();
            assert_eq!(unrelated_turn.invocation.unwrap().attempt_count, 1);
            release.send(()).unwrap();
            desktop_worker.join().unwrap();
        });
    }

    #[test]
    fn same_conversation_workers_follow_admission_order_not_thread_order() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let runtime = runtime(temporary.path(), "local");
        let first = runtime.admit_text(None, "@helper first").unwrap();
        let conversation_ref = first.conversation_ref().to_owned();
        let (first_started, first_starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        thread::scope(|scope| {
            let first_runtime = runtime.clone();
            let first_worker = scope.spawn(move || {
                first_runtime
                    .execute_admission(
                        &first,
                        observe_starts(&first_started, Some(&released)),
                    )
                    .unwrap()
            });
            first_starts.recv_timeout(Duration::from_secs(2)).unwrap();

            let second = runtime
                .admit_text(Some(&conversation_ref), "@helper second")
                .unwrap();
            let third = runtime
                .admit_text(Some(&conversation_ref), "@helper third")
                .unwrap();
            let (started, starts) = mpsc::channel();

            let third_runtime = runtime.clone();
            let third_started = started.clone();
            let third_admission = third.clone();
            let third_worker = scope.spawn(move || {
                third_runtime
                    .execute_admission(&third_admission, observe_starts(&third_started, None))
                    .unwrap()
            });
            let second_runtime = runtime.clone();
            let second_admission = second.clone();
            let second_worker = scope.spawn(move || {
                second_runtime
                    .execute_admission(&second_admission, observe_starts(&started, None))
                    .unwrap()
            });

            assert!(starts.recv_timeout(Duration::from_millis(80)).is_err());
            release.send(()).unwrap();
            first_worker.join().unwrap();
            assert_eq!(
                starts.recv_timeout(Duration::from_secs(2)).unwrap(),
                second.invocation_ref().unwrap()
            );
            assert_eq!(
                starts.recv_timeout(Duration::from_secs(2)).unwrap(),
                third.invocation_ref().unwrap()
            );
            second_worker.join().unwrap();
            third_worker.join().unwrap();
        });
    }

    #[test]
    fn local_execution_respects_the_configured_limit() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let runtime = runtime_with_options(
            temporary.path(),
            "local",
            LocalRuntimeOptions {
                max_concurrent_conversations: NonZeroUsize::new(4).unwrap(),
                ..Default::default()
            },
        );
        let admissions = (0..5)
            .map(|index| {
                runtime
                    .admit_text(None, format!("@helper conversation {index}"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (started, starts) = mpsc::channel();
        let mut releases = BTreeMap::new();
        thread::scope(|scope| {
            let mut workers = Vec::new();
            for admission in admissions {
                let runtime = runtime.clone();
                let started = started.clone();
                let invocation_ref = admission.invocation_ref().unwrap().to_owned();
                let (release, released) = mpsc::channel();
                releases.insert(invocation_ref, release);
                workers.push(scope.spawn(move || {
                    runtime
                        .execute_admission(
                            &admission,
                            observe_start_and_wait(started, released),
                        )
                        .unwrap()
                }));
            }

            let active = (0..4)
                .map(|_| starts.recv_timeout(Duration::from_secs(2)).unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(active.len(), 4);
            assert!(starts.recv_timeout(Duration::from_millis(80)).is_err());

            let released_ref = active.iter().next().unwrap();
            releases.get(released_ref).unwrap().send(()).unwrap();
            let final_ref = starts.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(!active.contains(&final_ref));

            for release in releases.values() {
                let _ = release.send(());
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }

    #[test]
    fn concurrent_workers_claim_different_recovered_conversations() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let seed = runtime(temporary.path(), "local");
        let first = seed.admit_text(None, "@helper recovered first").unwrap();
        let second = seed.admit_text(None, "@helper recovered second").unwrap();
        let first_ref = first.invocation_ref().unwrap().to_owned();
        let second_ref = second.invocation_ref().unwrap().to_owned();
        {
            let mut state = seed.lock().unwrap();
            assert!(matches!(
                state.admission.claim_invocation(&first_ref, 10).unwrap(),
                ConversationInvocationClaimOutcome::Claimed(_)
            ));
            assert!(matches!(
                state.admission.claim_invocation(&second_ref, 20).unwrap(),
                ConversationInvocationClaimOutcome::Claimed(_)
            ));
        }
        drop(seed);

        let runtime = runtime(temporary.path(), "local");
        let current_first = runtime.admit_text(None, "@helper current first").unwrap();
        let current_second = runtime.admit_text(None, "@helper current second").unwrap();
        let (first_started, first_starts) = mpsc::channel();
        let (release_first, released_first) = mpsc::channel();
        let (second_started, second_starts) = mpsc::channel();
        let (release_second, released_second) = mpsc::channel();

        thread::scope(|scope| {
            let first_runtime = runtime.clone();
            let recovered_refs = BTreeSet::from([first_ref.clone(), second_ref.clone()]);
            let first_worker = scope.spawn(move || {
                first_runtime
                    .execute_admission(
                        &current_first,
                        observe_selected_start_and_wait(
                            first_started,
                            released_first,
                            recovered_refs,
                        ),
                    )
                    .unwrap()
            });
            let first_claimed = first_starts.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(first_claimed == first_ref || first_claimed == second_ref);

            let second_runtime = runtime.clone();
            let recovered_refs = BTreeSet::from([first_ref.clone(), second_ref.clone()]);
            let second_worker = scope.spawn(move || {
                second_runtime
                    .execute_admission(
                        &current_second,
                        observe_selected_start_and_wait(
                            second_started,
                            released_second,
                            recovered_refs,
                        ),
                    )
                    .unwrap()
            });
            let second_claimed = second_starts.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(second_claimed == first_ref || second_claimed == second_ref);
            assert_ne!(second_claimed, first_claimed);

            release_first.send(()).unwrap();
            release_second.send(()).unwrap();
            first_worker.join().unwrap();
            second_worker.join().unwrap();
        });
    }

    #[test]
    fn pending_conversation_drain_leaves_unrelated_work_queued() {
        let temporary = tempfile::tempdir().unwrap();
        copy_workspace(fixture_workspace().parent().unwrap(), temporary.path());
        let runtime = runtime(temporary.path(), "local");
        let first = runtime.admit_text(None, "@helper first").unwrap();
        let second = runtime.admit_text(None, "@helper second").unwrap();
        let first_ref = first.invocation_ref().unwrap().to_owned();
        let second_ref = second.invocation_ref().unwrap().to_owned();
        let mut state = runtime.lock().unwrap();
        state.pending_invocation_refs = vec![first_ref.clone(), second_ref.clone()];

        let drained = take_ordered_pending_refs_for_group(
            &mut state,
            first.conversation_ref(),
        );

        assert_eq!(drained, vec![first_ref]);
        assert_eq!(state.pending_invocation_refs, vec![second_ref]);
    }
}
