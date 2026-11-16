import { assertEquals, assertRejects } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import { digest } from "./engine.ts";
import type {
  Installed,
  Occurrence,
  ScheduleRequest,
  Versioned,
} from "./schedule_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8797";
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const dir = await Deno.makeTempDir({ prefix: "cwpdb-schedules-" });
const hosted: Transport = async (body) => {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: "Bearer fictional-local-cwpdb-conformance-only",
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const envelope = await response.json();
  if (!envelope.ok) throw new Error(envelope.error);
  return envelope.result;
};
async function exercise(name: string, call: Transport) {
  const workspace = `cwp://fixture/workspaces/schedules-${crypto.randomUUID()}`;
  const context = {
    workspace_ref: workspace,
    caller_ref: `${workspace}/actors/host`,
  };
  const root = `${workspace}/runtime`;
  const scope = { context, runtime_root_ref: root };
  const initial = await call({
    operation: "read_conversation_graph",
    request: {
      scope,
      selection: { selection: "bootstrap" },
      continuation: null,
      page_size: 100,
      max_bytes: 1048576,
      expected_revision: null,
    },
  }) as { revision: string };
  const conversation = `${workspace}/conversation`;
  const invocation = `${workspace}/invocation`;
  await call({
    operation: "commit_conversation_graph",
    request: {
      scope,
      request_id: "schedule-execution",
      expected_revision: initial.revision,
      changes: {
        records: [{
          reference: conversation,
          kind: "conversation",
          properties: { title: "Scheduled" },
          owner_ref: null,
        }, {
          reference: invocation,
          kind: "invocation",
          properties: {
            record: { conversation_ref: conversation, state: "running" },
          },
          owner_ref: null,
        }],
        remove_records: [],
        memberships: [],
        publications: [],
        state_version: null,
        streaming_runs: null,
      },
    },
  });
  const run = <T>(schedule_request: ScheduleRequest) =>
    call({ operation: "schedule", context, schedule_request }) as Promise<T>;
  const owner = "fixture-installed-context";
  const definition = {
    declaration_ref: `${workspace}/declaration`,
    revision: "one",
  };
  const install = {
    runtime_root_ref: root,
    owner_context_ref: owner,
    author_actor_ref: context.caller_ref,
    definition,
    next_due_unix_ms: 2000,
  };
  let schedule = await run<Versioned<Installed>>({
    action: "install",
    request: install,
  });
  assertEquals(
    schedule.value.schedule_ref,
    `cwp://org.coworkerprotocol.local/installed-schedules/${await digest([
      owner,
      definition.declaration_ref,
    ])}`,
  );
  const replay = await run<Versioned<Installed>>({
    action: "install",
    request: install,
  });
  assertEquals(replay.value, schedule.value);
  schedule = replay;
  assertEquals(
    (await run<Versioned<Installed>[]>({
      action: "installed",
      runtime_root_ref: root,
      owner,
    })).map((v) => v.value.schedule_ref),
    [schedule.value.schedule_ref],
  );
  await assertRejects(() =>
    run({
      action: "get_schedule",
      reference: schedule.value.schedule_ref,
      owner: "foreign",
    })
  );
  await assertRejects(() =>
    run({
      action: "pause",
      owner,
      reference: schedule.value.schedule_ref,
      expected: schedule.revision + 99,
      paused: true,
      next_due_unix_ms: 3000,
    })
  );
  const paused = await run<Versioned<Installed>>({
    action: "pause",
    owner,
    reference: schedule.value.schedule_ref,
    expected: schedule.revision,
    paused: true,
    next_due_unix_ms: 3000,
  });
  assertEquals(paused.value.next_due_unix_ms, 2000);
  schedule = await run<Versioned<Installed>>({
    action: "pause",
    owner,
    reference: schedule.value.schedule_ref,
    expected: paused.revision,
    paused: false,
    next_due_unix_ms: 4000,
  });
  assertEquals(schedule.value.next_due_unix_ms, 4000);
  await assertRejects(() =>
    run({
      action: "availability",
      owner,
      changes: [{
        schedule_ref: schedule.value.schedule_ref,
        expected: schedule.revision,
        availability: "disabled",
        next_due_unix_ms: 6000,
      }, {
        schedule_ref: schedule.value.schedule_ref,
        expected: schedule.revision + 99,
        availability: "ready",
        next_due_unix_ms: 6000,
      }],
    })
  );
  assertEquals(
    await run({
      action: "get_schedule",
      reference: schedule.value.schedule_ref,
      owner,
    }),
    schedule,
  );
  const occurrenceRef =
    `cwp://org.coworkerprotocol.local/schedule-occurrences/${await digest([
      schedule.value.schedule_ref,
      "one",
      4000,
    ])}`;
  const occurrence: Occurrence = {
    occurrence_ref: occurrenceRef,
    schedule_ref: schedule.value.schedule_ref,
    definition,
    due_unix_ms: 4000,
    outcome: "pending",
    invocation_ref: null,
    conversation_ref: null,
    archived: false,
  };
  await assertRejects(() =>
    run({
      action: "reserve",
      request: {
        schedule_ref: schedule.value.schedule_ref,
        expected: schedule.revision,
        occurrence,
        next_due_unix_ms: 6000,
        active_occurrence_ref: `${workspace}/missing`,
      },
    })
  );
  assertEquals(
    await run({ action: "get_occurrence", reference: occurrenceRef }),
    null,
  );
  schedule = await run<Versioned<Installed>>({
    action: "reserve",
    request: {
      schedule_ref: schedule.value.schedule_ref,
      expected: schedule.revision,
      occurrence,
      next_due_unix_ms: 6000,
      active_occurrence_ref: occurrenceRef,
    },
  });
  let saved = await run<Versioned<Occurrence>>({
    action: "get_occurrence",
    reference: occurrenceRef,
  });
  await assertRejects(() =>
    run({
      action: "settle",
      request: {
        occurrence_ref: occurrenceRef,
        expected: saved.revision,
        outcome: "admitted",
        invocation_ref: invocation,
        conversation_ref: `${workspace}/missing`,
        archived: false,
      },
    })
  );
  assertEquals(
    await run({ action: "get_occurrence", reference: occurrenceRef }),
    saved,
  );
  saved = await run<Versioned<Occurrence>>({
    action: "settle",
    request: {
      occurrence_ref: occurrenceRef,
      expected: saved.revision,
      outcome: "terminal_success",
      invocation_ref: invocation,
      conversation_ref: conversation,
      archived: false,
    },
  });
  assertEquals(
    (await run<Versioned<Occurrence>[]>({
      action: "occurrences",
      reference: schedule.value.schedule_ref,
      selection: "UnarchivedTerminal",
    })).map((v) => v.value.occurrence_ref),
    [occurrenceRef],
  );
  assertEquals(
    (await run<[string, Versioned<Occurrence>][]>({
      action: "archive_candidates",
      runtime_root_ref: root,
    })).map((v) => v[0]),
    [context.caller_ref],
  );
  await assertRejects(() =>
    run({
      action: "settle",
      request: {
        occurrence_ref: occurrenceRef,
        expected: saved.revision,
        outcome: "pending",
        invocation_ref: invocation,
        conversation_ref: conversation,
        archived: false,
      },
    })
  );
  saved = await run<Versioned<Occurrence>>({
    action: "settle",
    request: {
      occurrence_ref: occurrenceRef,
      expected: saved.revision,
      outcome: "terminal_success",
      invocation_ref: invocation,
      conversation_ref: conversation,
      archived: true,
    },
  });
  assertEquals(
    await run({
      action: "occurrences",
      reference: schedule.value.schedule_ref,
      selection: "UnarchivedTerminal",
    }),
    [],
  );
  assertEquals(
    (await run<Versioned<Occurrence>[]>({
      action: "occurrences",
      reference: schedule.value.schedule_ref,
      selection: { Recent: 1 },
    }))[0],
    saved,
  );
  await assertRejects(() =>
    run({
      action: "occurrences",
      reference: schedule.value.schedule_ref,
      selection: { Recent: 101 },
    })
  );
  await assertRejects(() =>
    run({
      action: "install",
      request: {
        ...install,
        definition: { ...definition, extra: "changed without revision" },
      },
    })
  );
  schedule = await run<Versioned<Installed>>({
    action: "install",
    request: {
      ...install,
      definition: { ...definition, revision: "two" },
      next_due_unix_ms: 8000,
    },
  });
  assertEquals(schedule.value.active_occurrence_ref, occurrenceRef);
  assertEquals(schedule.value.next_due_unix_ms, 6000);
  const secondOccurrence = {
    ...occurrence,
    occurrence_ref:
      `cwp://org.coworkerprotocol.local/schedule-occurrences/${await digest([
        schedule.value.schedule_ref,
        "two",
        4000,
      ])}`,
    definition: { ...definition, revision: "two" },
  };
  await run({
    action: "reserve",
    request: {
      schedule_ref: schedule.value.schedule_ref,
      expected: schedule.revision,
      occurrence: secondOccurrence,
      next_due_unix_ms: 10000,
      active_occurrence_ref: secondOccurrence.occurrence_ref,
    },
  });
  assertEquals(
    (await run<Versioned<Occurrence>[]>({
      action: "occurrences",
      reference: schedule.value.schedule_ref,
      selection: { Recent: 2 },
    })).map((record) => record.value.occurrence_ref),
    [secondOccurrence.occurrence_ref, occurrenceRef],
  );
  console.log(JSON.stringify({
    backend: name,
    checks: 15,
    result: "pass",
    covers: [
      "deterministic-install-replay",
      "owner-isolation",
      "revision-CAS",
      "pause-resume",
      "atomic-availability",
      "atomic-reservation",
      "endpoint-link-rollback",
      "terminal-monotonicity",
      "retained-execution-links",
      "archive-index",
      "recent-index",
      "finite-page",
      "declaration-revision-reuse",
      "active-revision-update",
      "same-due-retained-order",
    ],
  }));
}
const native = await processTransport([`${dir}/store.db`], binary);
const remote = await processTransport(["--remote", endpoint], binary);
try {
  await exercise("native-canonical-schedules", native.call);
  await exercise("actual-local-workerd-schedules", hosted);
  await exercise("rust-remote-schedules-to-workerd", remote.call);
} finally {
  await native.close();
  await remote.close();
  await Deno.remove(dir, { recursive: true });
}
