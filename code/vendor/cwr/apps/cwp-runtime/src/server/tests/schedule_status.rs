use super::*;
use cwr::v2026_09::{
    host::ConversationTurnState,
    invocation::ActionInvocationState,
    local::{route_action_target, ScheduleDeclaration},
    upstream::ConversationContextPolicy,
};

const TEST_NAME: &str =
    "server::tests::schedule_status::timer_admission_publishes_pending_status_while_execution_capacity_is_held";
const CHILD_ENV: &str = "CWR_SCHEDULE_STATUS_TEST_CHILD";

#[test]
fn timer_admission_publishes_pending_status_while_execution_capacity_is_held() {
    if !host_admission_status::run_isolated_test(TEST_NAME, CHILD_ENV) {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let runtime = host_admission_status::fixture_runtime(temporary.path());
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let blocker = runtime
        .admit_action(None, "@helper--run--task", serde_json::json!({}))
        .unwrap();
    let reservation = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        reservation.record().invocation_ref,
        blocker.invocation_ref().unwrap()
    );
    let owner = "cwp://example.test/contexts/timer-fixture";
    let schedule = install_due_schedule(&runtime, &person, owner);
    let scaffold = tempfile::tempdir().unwrap();
    let state = runtime_state(&scaffold);
    lock_state(&state).unwrap().runtime = runtime.clone();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let (client, server) = wire::wire_pair();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    // The guard fences claims before the held reservation is dropped, including
    // on test failure. No synthetic adapter should execute in this fixture.
    let guard = SchedulerGuard {
        state: Arc::clone(&state),
        runtime: runtime.clone(),
    };
    scheduler::start(&state).unwrap();
    let created = observe_scheduled_pending(client, blocker.conversation_ref());
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    let runs = runtime
        .schedules_module_as(owner, &person)
        .unwrap()
        .recent_runs(&schedule, 10)
        .unwrap();
    let run = runs
        .iter()
        .find(|run| run.conversation_ref.as_deref() == Some(created.as_str()))
        .unwrap();
    let admission = runtime
        .admission_record(
            "cwp://org.coworkerprotocol/cwr/ingress/schedules",
            &run.occurrence_ref,
        )
        .unwrap()
        .unwrap();
    let record = runtime.turn(&admission).unwrap().invocation.unwrap();
    assert_eq!(record.state, ActionInvocationState::Admitted);
    assert_eq!(record.started_at_unix_ms, None);
    assert_eq!(
        lock_state(&state).unwrap().published_turn_statuses,
        runtime.conversation_host().turn_statuses().unwrap()
    );
    drop(guard);
    disconnect_writer(&writer);
    drop(reservation);
}

fn install_due_schedule(runtime: &LocalRuntime, person: &str, owner: &str) -> String {
    let graph = runtime.graph().unwrap();
    let catalog = runtime.catalog().unwrap();
    let helper = catalog
        .known_actors
        .iter()
        .find(|actor| actor.handle == "@helper")
        .unwrap();
    let route = graph
        .routes
        .iter()
        .find(|route| route.actor_ref == helper.actor_ref && route.action_name == "run--task")
        .unwrap();
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: vec![],
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([
                (
                    "workspace".into(),
                    catalog
                        .known_actors
                        .iter()
                        .map(|actor| actor.actor_ref.clone())
                        .collect(),
                ),
                (owner.into(), vec![helper.actor_ref.clone()]),
            ]),
        }))
        .unwrap();
    let now = current_unix_ms();
    // Initialize the timer before installation so first-boot catch-up rules do
    // not deliberately skip this fixture's newly due occurrence.
    assert!(runtime.tick_schedules(now).unwrap().is_empty());
    runtime
        .schedules_module_as(owner, person)
        .unwrap()
        .install(
            ScheduleDeclaration {
                declaration_ref: "cwp://example.test/releases/fixture/schedules/pending".into(),
                revision: "v1".into(),
                target: route_action_target(&graph.runtime_ref, route),
                input: serde_json::json!({}),
                anchor_unix_ms: now,
                interval_ms: 60_000,
            },
            now - 1,
        )
        .unwrap()
        .schedule_ref
}

fn observe_scheduled_pending(client: UnixStream, blocker: &str) -> String {
    let mut created = None;
    for line in BufReader::new(client).lines() {
        let event: RuntimeServiceEvent = serde_json::from_str(&line.unwrap()).unwrap();
        match event {
            RuntimeServiceEvent::ConversationUpsert { conversation }
                if conversation.conversation_ref != blocker =>
            {
                created = Some(conversation.conversation_ref);
            }
            RuntimeServiceEvent::ConversationTurnStatusSnapshot {
                conversation_ref,
                turns,
            } if conversation_ref != blocker && !turns.is_empty() => {
                assert_eq!(
                    created.as_deref(),
                    Some(conversation_ref.as_str()),
                    "scheduled conversation metadata must precede its status"
                );
                assert_eq!(turns[0].state, ConversationTurnState::Pending);
                assert_eq!(turns[0].started_at_unix_ms, None);
                return conversation_ref;
            }
            _ => {}
        }
    }
    panic!("scheduled admission ended without a pending status snapshot")
}

struct SchedulerGuard {
    state: SharedState,
    runtime: LocalRuntime,
}
impl Drop for SchedulerGuard {
    fn drop(&mut self) {
        self.state.scheduler.stop();
        let _ = self.runtime.shutdown(Duration::from_secs(1));
    }
}
