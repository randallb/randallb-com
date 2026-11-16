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
        state_version: null,
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
    }

    const history = (
      selection: Record<string, unknown>,
      extra: Record<string, unknown> = {},
    ) => ({
      operation: "conversation_history_read",
      request: {
        scope,
        conversation_ref: conversation,
        selection,
        expected_commit_revision: null,
        participant_actor_ref: actor,
        include_execution: false,
        deadline_unix_ms: Date.now() + 30000,
        max_bytes: 24 * 1024 * 1024,
        ...extra,
      },
    });
    const equalHistory = async (body: unknown) => {
      const result = await native.call(body);
      assertEquals(await worker(body), result);
      assertEquals(await remote.call(body), result);
      checks++;
      return result;
    };
    const first = await equalHistory(
      history({
        kind: "page",
        after: null,
        before: null,
        first: 3,
        last: null,
      }),
    ) as {
      revision: string;
      commit_revision: string;
      items: { position: number; message_ref: string }[];
      total_count: number;
    };
    assertEquals(first.items.length, 3);
    assertEquals(first.total_count, canonical ? 5 : 7);
    const cursor = {
      position: first.items[0].position,
      revision: canonical ? first.revision : null,
    };
    const links = await equalHistory(
      history({
        kind: "replies",
        message_ref: message(0),
        after_message_ref: null,
        after_revision: null,
        first: 1,
      }),
    ) as { reply_message_refs: string[]; has_next_page: boolean };
    assertEquals(links.reply_message_refs, [message(3)]);
    assertEquals(links.has_next_page, true);
    const more = await equalHistory(
      history({
        kind: "replies",
        message_ref: message(0),
        after_message_ref: message(3),
        after_revision: first.revision,
        first: 1,
      }),
    ) as { reply_message_refs: string[] };
    assertEquals(more.reply_message_refs, [message(5)]);
    const origin = await equalHistory(
      history({ kind: "message", message_ref: message(3) }),
    ) as { items: { origin_invocation_ref: string | null }[] };
    assertEquals(origin.items[0].origin_invocation_ref, invocation(1));
    for (
      const selection of [
        { kind: "page", after: cursor, before: null, first: 2, last: null },
        { kind: "page", after: null, before: null, first: null, last: 2 },
        { kind: "page", after: null, before: null, first: 4, last: 2 },
        {
          kind: "archive_page",
          after: null,
          before: null,
          first: 3,
          last: null,
        },
        { kind: "page", after: null, before: null, first: 0, last: null },
        {
          kind: "page_positions",
          after: null,
          before: null,
          first: 3,
          last: null,
        },
        {
          kind: "scan_positions",
          after_position: null,
          after_revision: null,
          first: 3,
        },
        { kind: "retained_message", message_ref: message(0) },
        { kind: "archive_message", message_ref: message(0) },
        { kind: "message", message_ref: message(0) },
        {
          kind: "replies",
          message_ref: message(0),
          after_message_ref: null,
          after_revision: null,
          first: 2,
        },
        { kind: "scan", after_position: null, after_revision: null, first: 2 },
        {
          kind: "scan",
          after_position: first.items[0].position,
          after_revision: first.revision,
          first: 2,
        },
      ]
    ) await equalHistory(history(selection));
    for (const call of [native.call, worker, remote.call]) {
      for (
        const extra of [
          { participant_actor_ref: base + "/foreign" },
          { expected_commit_revision: "stale" },
          { deadline_unix_ms: 1 },
          { max_bytes: 1 },
        ]
      ) {
        await assertRejects(() =>
          call(history({ kind: "message", message_ref: message(0) }, extra))
        );
      }
      await assertRejects(() =>
        call(history({ kind: "message", message_ref: base + "/missing" }))
      );
      await assertRejects(() =>
        call(
          history({
            kind: "page",
            after: { position: 999, revision: first.revision },
            before: null,
            first: 2,
            last: null,
          }),
        )
      );
      if (canonical) {
        await assertRejects(() =>
          call(
            history({
              kind: "scan",
              after_position: 0,
              after_revision: "stale",
              first: 2,
            }),
          )
        );
      }
    }
    for (const call of [native.call, worker]) {
      const { revision } = await call(bootstrap) as { revision: string };
      await call({
        operation: "commit_conversation_graph",
        request: {
          scope,
          request_id: "history-concurrent-change",
          expected_revision: revision,
          changes: {
            records: [{
              reference: base + "/unrelated",
              kind: "message",
              properties: {
                record: {
                  message_ref: base + "/unrelated",
                  text: "intervening write",
                },
              },
              owner_ref: null,
            }],
            memberships: [],
            remove_records: [],
            publications: [],
            state_version: null,
            streaming_runs: null,
            repair_legacy: false,
          },
          journal: null,
          authority_guards: [],
        },
      });
    }
    for (const call of [native.call, worker, remote.call]) {
      await assertRejects(() =>
        call(
          history({ kind: "message", message_ref: message(0) }, {
            expected_commit_revision: first.commit_revision,
          }),
        )
      );
      await assertRejects(() =>
        call(history({ kind: "message", message_ref: message(6) }))
      );
    }
    console.log(
      `${
        canonical ? "canonical" : "legacy"
      } history Native/workerd/Remote snapshot/origin/page/scan/refusal parity PASS`,
    );
  }
  console.log(
    `${checks} exact history facts plus foreign/stale/deadline/byte/cursor refusals PASS`,
  );
} finally {
  for (const native of natives) await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
