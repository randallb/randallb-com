/** Local fictional Native / actual workerd / Rust RemoteStore page parity. */
import { assertEquals, assertRejects } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import type {
  MembershipChange,
  Mutation,
  Scope,
  WriteRecord,
} from "./conversation_types.ts";
import type {
  CompletionPageFacts,
  CompletionPageRequest,
  ContentPageFacts,
  ContentPageRequest,
} from "./conversation_page_types.ts";
const binary = Deno.args[0],
  endpoint = Deno.args[1] ?? "http://127.0.0.1:18879";
if (!binary) throw new Error("native fixture binary required");
const directory = await Deno.makeTempDir({ prefix: "cwpdb-timeline-" });

const remote = await processTransport(
  ["--remote", endpoint],
  binary,
);
const worker = async (body: unknown): Promise<unknown> => {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: "Bearer fictional-local-cwpdb-conformance-only",
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const value = await response.json();
  if (!value.ok) throw new Error(value.error);
  return value.result;
};
type Call = (body: unknown) => Promise<unknown>;
let checks = 0;
const natives: { close: () => Promise<void> }[] = [];
try {
  for (const canonical of [false, true]) {
    const native = await processTransport([
      `${directory}/native-${canonical}.sqlite`,
    ], binary);
    natives.push(native);
    const base = `cwp://fixture/pages-${crypto.randomUUID()}`,
      conversation = `${base}/conversation`,
      root = `${base}/root`,
      child = `${base}/child`,
      actor = `${base}/actor`;
    const scope: Scope = {
      context: { workspace_ref: base, caller_ref: `${base}/host` },
      runtime_root_ref: `${base}/runtime`,
    };
    const message = (n: number) => `${base}/m${n}`,
      item = (n: number) => `${base}/i${n}`,
      invocation = (n: number) => `${base}/inv${n}`;
    const records: WriteRecord[] = [], memberships: MembershipChange[] = [];
    const record = (
      reference: string,
      kind: WriteRecord["kind"],
      properties: Record<string, unknown>,
      owned = false,
    ) =>
      records.push({
        reference,
        kind,
        properties: kind === "conversation_timeline_item"
          ? { publication_ref: `${reference}/publication`, ...properties }
          : properties,
        owner_ref: owned ? conversation : null,
      });
    const member = (
      source_ref: string,
      kind: string,
      target_ref: string,
      position = 0,
      properties = {},
    ) =>
      memberships.push({
        operation: "upsert",
        source_ref,
        kind,
        member: { target_ref, position, properties },
      });
    record(conversation, "conversation", {
      owner_context_ref: base,
    });
    member(`${scope.runtime_root_ref}/cwpdb`, "conversation", conversation);
    member(conversation, "participant", actor);
    for (let n = 0; n < 7; n++) {
      record(
        message(n),
        "message",
        canonical && n === 2
          ? {
            streaming: {
              conversation_ref: conversation,
              message: { message_ref: message(n), text: "draft" },
            },
          }
          : {
            record: {
              message_ref: message(n),
              author_actor_ref: actor,
              addressed_actor_refs: [],
              text: `text ${n} 🌵`,
              ...(n === 1
                ? { progress: { invocation_ref: invocation(1) } }
                : {}),
              ...([3, 5].includes(n)
                ? { reply_to_message_ref: message(0) }
                : {}),
            },
            ...(n === 2
              ? {
                streaming: {
                  conversation_ref: conversation,
                  message: { message_ref: message(n), text: "draft" },
                },
              }
              : {}),
          },
      );
      member(
        conversation,
        "message",
        message(n),
        n,
        n === 0 ? { visualizations: [{ fixture: true }] } : {},
      );
    }
    for (let n = 1; n <= 3; n++) {
      record(invocation(n), "invocation", {
        record: {
          invocation_ref: invocation(n),
          conversation_ref: conversation,
          state: "completed",
          ...(n === 1 ? { reply_message_ref: message(3) } : {}),
        },
      });
    }
    record(`${base}/activity`, "activity", {
      record: {
        activity_ref: `${base}/activity`,
        sequence: 1,
        occurred_at_unix_ms: 123,
        event: { kind: "message_recorded", message_ref: message(0) },
      },
    });
    member(conversation, "activity", `${base}/activity`, 0);
    if (canonical) {
      record(root, "conversation_timeline_connection", {
        revision: 3,
        store_sequence: 0,
        next_sequence: 8,
      }, true);
      record(child, "conversation_timeline_connection", {
        revision: 5,
        store_sequence: 1,
        next_sequence: 2,
      }, true);
      member(conversation, "conversation_timeline_root", root);
      for (let n = 0; n < 6; n++) {
        record(item(n), "conversation_timeline_item", {}, true);
        member(root, "conversation_timeline_item", item(n), n);
        member(item(n), "conversation_timeline_message", message(n));
      }
      member(item(0), "conversation_timeline_child", child);
      record(`${base}/nested`, "conversation_timeline_item", {}, true);
      member(child, "conversation_timeline_item", `${base}/nested`, 0);
      member(`${base}/nested`, "conversation_timeline_message", message(0));
      for (const n of [1, 3]) {
        record(`${base}/invitem${n}`, "conversation_timeline_item", {}, true);
        member(
          root,
          "conversation_timeline_item",
          `${base}/invitem${n}`,
          10 + n,
        );
        member(
          `${base}/invitem${n}`,
          "conversation_timeline_invocation",
          invocation(n),
        );
      }
    }
    const bootstrap = {
      operation: "read_conversation_graph",
      request: {
        scope,
        selection: { selection: "bootstrap" },
        continuation: null,
        page_size: 100,
        max_bytes: 1048576,
        expected_revision: null,
      },
    };
    for (const call of [native.call, worker]) {
      const { revision } = await call(bootstrap) as { revision: string };
      const changes: Mutation["changes"] = {
        records,
        memberships,
        remove_records: [],
        publications: [{
          conversation_ref: conversation,
          from_position: 0,
          items: [
            ...Array.from(
              { length: 6 },
              (_, n) => ({
                kind: "message" as const,
                message_ref: message(n),
                position: n,
              }),
            ),
            {
              kind: "turn" as const,
              position: 6,
              invocation_ref: invocation(1),
            },
            {
              kind: "turn" as const,
              position: 7,
              invocation_ref: invocation(3),
            },
          ],
          activities: [
            {
              activity_ref: `${base}/projected/one`,
              position: 1,
              record: {
                activity_ref: `${base}/projected/one`,
                sequence: 1,
                occurred_at_unix_ms: 123,
                event: {
                  kind: "work_lifecycle",
                  invocation_ref: invocation(1),
                  source_message_ref: message(0),
                  parent_invocation_ref: null,
                  state: "completed",
                },
              },
            },
            {
              activity_ref: `${base}/projected/three`,
              position: 3,
              record: {
                activity_ref: `${base}/projected/three`,
                sequence: 3,
                occurred_at_unix_ms: 125,
                event: {
                  kind: "work_update",
                  invocation_ref: invocation(3),
                  source_message_ref: message(3),
                  parent_invocation_ref: invocation(1),
                  text: "nested",
                  activity: null,
                  wait: null,
                },
              },
            },
          ],
          remove_activities: [],
        }],
        state_version: canonical
          ? "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1"
          : null,
        streaming_runs: null,
        repair_legacy: false,
      };
      await call({
        operation: "commit_conversation_graph",
        request: {
          scope,
          request_id: "page-seed",
          expected_revision: revision,
          changes,
          journal: null,
          authority_guards: [],
        },
      });
      // The normal writer clears streaming when a retained record is supplied
      // in the same mutation. A later streaming-only update is the real overlap.
      if (!canonical) {
        const { revision: afterSeed } = await call(bootstrap) as {
          revision: string;
        };
        await call({
          operation: "commit_conversation_graph",
          request: {
            scope,
            request_id: "later-stream",
            expected_revision: afterSeed,
            changes: {
              ...changes,
              records: [{
                reference: message(2),
                kind: "message",
                owner_ref: null,
                properties: {
                  streaming: {
                    conversation_ref: conversation,
                    message: {
                      message_ref: message(2),
                      text: "later live draft",
                    },
                  },
                },
              }],
              memberships: [],
              publications: [],
              state_version: null,
            },
            journal: null,
            authority_guards: [],
          },
        });
      }
    }

    const select = (
      selection: Record<string, unknown>,
      extra: Record<string, unknown> = {},
    ) => ({
      operation: "read_conversation_selection",
      request: {
        scope,
        selection,
        expected_commit_revision: commit,
        deadline_unix_ms: Date.now() + 30000,
        max_bytes: 24 * 1024 * 1024,
        ...extra,
      },
    });
    let commit: string | null = null;
    const initial = await native.call(select({ operation: "position" })) as {
      commit_revision: string;
    };
    commit = initial.commit_revision;
    const position = await worker(select({ operation: "position" })) as {
      commit_revision: string;
    };
    assertEquals(position.commit_revision, commit);
    const equal = async (selection: Record<string, unknown>) => {
      const expected = await native.call(select(selection));
      assertEquals(await worker(select(selection)), expected);
      assertEquals(await remote.call(select(selection)), expected);
      checks++;
      return (expected as { result: unknown }).result;
    };
    const window = { after: null, before: null, first: 3, last: null };
    const content = await equal({
      operation: "content_positions",
      conversation_ref: conversation,
      window,
      include_execution: false,
    }) as { items: { record: unknown }[]; total_count: number };
    assertEquals(content.total_count, canonical ? 4 : 6);
    assertEquals(content.items.length, 3);
    assertEquals(content.items[0].record, null);
    const point = await equal({
      operation: "content_message",
      conversation_ref: conversation,
      message_ref: message(0),
      include_execution: false,
    }) as { record: { text: string }; occurred_at_unix_ms: number | null };
    assertEquals(point.record.text, "text 0 🌵");
    assertEquals(point.occurred_at_unix_ms, 123);
    if (!canonical) {
      const retained = await equal({
        operation: "content_message",
        conversation_ref: conversation,
        message_ref: message(2),
        include_execution: false,
      }) as { record: { text: string } };
      assertEquals(retained.record.text, "text 2 🌵");
    }
    await equal({
      operation: "completion_page",
      conversation_ref: conversation,
      window: { after: 0, through: null, revision: null, first: 2 },
      include_execution: false,
    });
    const timeline = await equal({
      operation: "timeline_page",
      conversation_ref: conversation,
      window,
      include_execution: false,
    }) as { anchors: { position: number }[]; revision: string };
    assertEquals(timeline.anchors.length, 3);
    if (!canonical) {
      const legacy = await equal({
        operation: "legacy_timeline_page",
        conversation_ref: conversation,
        window,
        include_execution: false,
      }) as { anchors: { position: number }[]; revision: string };
      await equal({
        operation: "legacy_timeline_message",
        conversation_ref: conversation,
        key: {
          position: legacy.anchors[0].position,
          revision: legacy.revision,
        },
        message_ref: message(0),
        include_execution: false,
      });
      for (
        const bounds of [
          {
            after: { position: 0, revision: legacy.revision },
            before: null,
            first: 2,
            last: null,
          },
          { after: null, before: null, first: null, last: 2 },
          {
            after: { position: 7, revision: legacy.revision },
            before: null,
            first: 2,
            last: null,
          },
        ]
      ) {
        await equal({
          operation: "legacy_timeline_page",
          conversation_ref: conversation,
          window: bounds,
          include_execution: false,
        });
      }
      for (const call of [native.call, worker, remote.call]) {
        await assertRejects(() =>
          call(
            select({
              operation: "legacy_timeline_page",
              conversation_ref: conversation,
              window: { ...window, after: { position: 0, revision: "stale" } },
              include_execution: false,
            }),
          )
        );
        await assertRejects(() =>
          call(
            select({
              operation: "legacy_timeline_page",
              conversation_ref: conversation,
              window: { ...window, first: 0 },
              include_execution: false,
            }),
          )
        );
      }
    } else {for (const call of [native.call, worker, remote.call]) {
        await assertRejects(() =>
          call(
            select({
              operation: "legacy_timeline_page",
              conversation_ref: conversation,
              window,
              include_execution: false,
            }),
          )
        );
      }}
    for (const call of [native.call, worker, remote.call]) {
      for (
        const extra of [{ expected_commit_revision: "stale" }, {
          deadline_unix_ms: 1,
        }, { max_bytes: 1 }]
      ) {
        await assertRejects(() =>
          call(
            select({
              operation: "content_positions",
              conversation_ref: conversation,
              window,
              include_execution: false,
            }, extra),
          )
        );
      }
    }
    console.log(
      `${
        canonical ? "canonical" : "legacy"
      } indexed Native/workerd/Remote finite selection exact parity PASS`,
    );
  }
  console.log(
    `${checks} exact indexed selection results plus layout/cursor/stale/deadline/byte refusals PASS`,
  );
} finally {
  for (const native of natives) await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
