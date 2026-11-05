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
const directory = await Deno.makeTempDir({ prefix: "cwpdb-pages-" });

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
              ...(n === 1 ? { progress: {} } : {}),
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
        publications: [],
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
    const calls: Call[] = [native.call, worker, remote.call];
    const content = (extra: Partial<ContentPageRequest> = {}) => ({
      operation: "conversation_content_page",
      request: {
        scope,
        conversation_ref: conversation,
        after: null,
        before: null,
        first: 3,
        last: null,
        max_bytes: 1048576,
        ...extra,
      },
    });
    const completion = (extra: Partial<CompletionPageRequest> = {}) => ({
      operation: "conversation_completion_page",
      request: {
        scope,
        conversation_ref: conversation,
        after_sequence: 0,
        through_sequence: null,
        cursor_revision: null,
        first: 1,
        max_bytes: 1048576,
        ...extra,
      },
    });
    const equal = async (body: unknown) => {
      const expected = await native.call(body);
      assertEquals(await worker(body), expected);
      assertEquals(await remote.call(body), expected);
      checks++;
      return expected;
    };
    const first = await equal(content()) as ContentPageFacts;
    for (
      const extra of [
        { first: 0 },
        { first: null, last: 0 },
        { first: null, last: 2 },
        { first: 4, last: 1 },
        { first: 1, last: 4 },
      ]
    ) await equal(content(extra));
    const after = {
        position: first.items[0].position,
        revision: first.revision,
      },
      before = {
        position: first.items.at(-1)!.position,
        revision: first.revision,
      };
    await equal(content({ after }));
    await equal(content({ before }));
    await equal(content({ after, before, first: 1 }));
    for (const call of calls) {
      await assertRejects(() =>
        call(content({ after: { position: 999, revision: first.revision } }))
      );
      await assertRejects(() => call(content({ max_bytes: 1 })));
      if (canonical) {
        await assertRejects(() =>
          call(content({ after: { ...after, revision: "stale" } }))
        );
      }
    }
    const completions = await equal(completion()) as CompletionPageFacts;
    await equal(
      completion({
        after_sequence: completions.next_after,
        through_sequence: completions.through_sequence,
        cursor_revision: completions.revision,
      }),
    );
    await equal(completion({ through_sequence: 1, first: 200 }));
    await equal(
      completion({
        after_sequence: 3,
        through_sequence: 3,
        cursor_revision: completions.revision,
      }),
    );
    for (const call of calls) {
      await assertRejects(() => call(completion({ through_sequence: -1 })));
      if (canonical) {
        await assertRejects(() =>
          call(completion({ after_sequence: 1, cursor_revision: "stale" }))
        );
      }
    }
    console.log(
      `${
        canonical ? "canonical" : "legacy"
      } Native/workerd/Remote page parity PASS`,
    );
  }
  console.log(
    `${checks} exact page results plus invalid/stale/budget refusal cases PASS`,
  );
} finally {
  for (const native of natives) await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
