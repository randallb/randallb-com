import { readSelection } from "./read_selection.ts";
import type { ReadSelectionRequest } from "./read_selection_types.ts";
import {
  historyRead,
  type HistoryReadRequest,
} from "./conversation_history_read.ts";
import {
  timelinePage,
  type TimelineReadRequest,
} from "./conversation_timeline_read.ts";
import { completionPage, contentPage } from "./conversation_pages.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
import type {
  CompletionPageRequest,
  ContentPageRequest,
} from "./conversation_page_types.ts";
import { ResourceReader } from "./conversation_resource.ts";
import {
  timelineMemberships,
  timelineRecord,
} from "./conversation_timeline_write.ts";
import { containsMember, historyStatus } from "./conversation_history.ts";
import {
  type Authorizer,
  canonical,
  digest,
  type Storage,
  type Transaction,
  validContext,
} from "./engine.ts";
import { Invocations } from "./invocations.ts";
import {
  addDependency,
  Graph,
  rootRef,
  STATE,
  TIMELINE_STATE,
} from "./conversation_graph.ts";
import { readPage } from "./conversation_read.ts";
import { publish } from "./conversation_publications.ts";
import { Writer } from "./conversation_write.ts";
import { repair } from "./conversation_maintenance.ts";
import {
  MAX_WIRE_BYTES,
  type Mutation,
  type Outcome,
  type ReadRequest,
  type Scope,
} from "./conversation_types.ts";
interface SavedReceipt {
  digest?: string;
  outcome?: Outcome;
}
import { summary as summaryFacts } from "./conversation_summary.ts";
export class Conversations {
  async readSelection(request: ReadSelectionRequest) {
    validContext(request.scope.context, request.scope.runtime_root_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.scope.runtime_root_ref,
        false,
      );
      const result = await readSelection(
        new Graph(tx, request.scope),
        request,
        this.authorize,
      );
      await this.authorize(
        request.scope.context,
        request.scope.runtime_root_ref,
        false,
      );
      return result;
    });
  }

  async historyRead(request: HistoryReadRequest) {
    validContext(request.scope.context, request.conversation_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      const result = await historyRead(new Graph(tx, request.scope), request);
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      if (Date.now() >= request.deadline_unix_ms) {
        throw new Error("conversation read operation exceeded its deadline");
      }
      return result;
    });
  }

  async timelinePage(request: TimelineReadRequest) {
    validContext(request.scope.context, request.conversation_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      const result = await timelinePage(new Graph(tx, request.scope), request);
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      if (Date.now() >= request.deadline_unix_ms) {
        throw new Error("conversation read operation exceeded its deadline");
      }
      return result;
    });
  }

  async contentPage(request: ContentPageRequest) {
    validContext(request.scope.context, request.conversation_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      const result = await contentPage(
        new PageReader(new Graph(tx, request.scope)),
        request,
      );
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      return result;
    });
  }
  async completionPage(request: CompletionPageRequest) {
    validContext(request.scope.context, request.conversation_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      const result = await completionPage(
        new PageReader(new Graph(tx, request.scope)),
        request,
      );
      await this.authorize(
        request.scope.context,
        request.conversation_ref,
        false,
      );
      return result;
    });
  }

  async resource(
    scope: Scope,
    reference: string,
  ): Promise<import("./conversation_types.ts").ResourceFacts | null> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const result = await new ResourceReader(new Graph(tx, scope)).facts(
        reference,
        this.authorize,
      );
      await this.authorize(scope.context, reference, false);
      return result;
    });
  }

  async historyStatus(
    scope: Scope,
    reference: string,
  ): Promise<import("./conversation_types.ts").HistoryStatus | null> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const result = await historyStatus(new Graph(tx, scope), reference);
      await this.authorize(scope.context, reference, false);
      return result;
    });
  }
  async containsMember(
    scope: Scope,
    source: string,
    kind: string,
    target: string,
  ): Promise<boolean> {
    validContext(scope.context, source);
    validContext(scope.context, target);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, source, false);
      await this.authorize(scope.context, target, false);
      const result = await containsMember(
        new Graph(tx, scope),
        source,
        kind,
        target,
      );
      await this.authorize(scope.context, source, false);
      await this.authorize(scope.context, target, false);
      return result;
    });
  }

  async endpointKinds(
    scope: Scope,
    reference: string,
  ): Promise<{ node_kind: string | null; edge_kind: string | null }> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const graph = new Graph(tx, scope);
      await graph.ensure();
      return await graph.endpointKinds(reference);
    });
  }
  async summary(
    scope: Scope,
    reference: string,
  ): Promise<import("./conversation_types.ts").SummaryFacts | null> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const result = await summaryFacts(new Graph(tx, scope), reference);
      await this.authorize(scope.context, reference, false);
      return result;
    });
  }

  constructor(
    private readonly storage: Storage,
    private readonly authorize: Authorizer,
  ) {}
  async identityInUse(scope: Scope, reference: string): Promise<boolean> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      await new Graph(tx, scope).ensure();
      return (await tx.get(`identity:${reference}`)) !== undefined;
    });
  }
  async retainedOrder(scope: Scope, reference: string): Promise<number | null> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      await new Graph(tx, scope).ensure();
      return await tx.get<number>(`ordinal:${reference}`) ?? null;
    });
  }
  async publishedProgress(
    scope: Scope,
    reference: string,
  ): Promise<string | null> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const graph = new Graph(tx, scope);
      await graph.ensure();
      for (const lookup of await graph.incident(reference)) {
        if (
          lookup.kind !== "field:progress.source_activity_ref" ||
          lookup.target_ref !== reference
        ) continue;
        for (const edge of await graph.incident(lookup.source_ref)) {
          if (
            edge.kind === "message" && edge.target_ref === lookup.source_ref
          ) {
            await this.authorize(scope.context, edge.source_ref, false);
            return edge.source_ref;
          }
        }
      }
      return null;
    });
  }
  async read(request: ReadRequest): Promise<unknown> {
    if (new TextEncoder().encode(canonical(request)).length > MAX_WIRE_BYTES) {
      throw new Error("CWR read request exceeds24MiB wire budget");
    }
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.selection.selection === "conversation" ||
          (request.selection.selection === "conversation_metadata" &&
            request.selection.conversation_ref)
          ? request.selection.conversation_ref!
          : request.scope.runtime_root_ref,
        false,
      );
      return await readPage(new Graph(tx, request.scope), request);
    });
  }
  async get(scope: Scope, reference: string): Promise<unknown> {
    validContext(scope.context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(scope.context, reference, false);
      const graph = new Graph(tx, scope);
      await graph.ensure();
      return await graph.get(reference);
    });
  }
  async reconcile(input: Mutation): Promise<Outcome | null> {
    const request = normalize(input);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.scope.runtime_root_ref,
        true,
      );
      await new Graph(tx, request.scope).ensure();
      return await reconcile(tx, request);
    });
  }
  async commit(input: Mutation): Promise<Outcome> {
    const request = normalize(input);
    const changes = request.changes;
    if (
      changes.records.length + changes.remove_records.length +
            changes.memberships.length + (changes.timeline_items?.length ?? 0) >
        50000 ||
      new TextEncoder().encode(canonical(request)).length > MAX_WIRE_BYTES
    ) throw new Error("CWR runtime mutation exceeds its work/byte budget");
    return await this.storage.transaction(async (tx) => {
      await this.authorize(
        request.scope.context,
        request.scope.runtime_root_ref,
        true,
      );
      const graph = new Graph(tx, request.scope);
      const root = await graph.ensure();
      const replay = await reconcile(tx, request);
      if (replay) return replay;
      const counter = Number(root.properties.commit_revision);
      const revision = await digest([rootRef(request.scope), counter]);
      if (revision !== request.expected_revision) {
        throw new Error("conversation revision_conflict");
      }
      for (const guard of request.authority_guards ?? []) {
        const node = await tx.get<
          { workspace: string; record: { revision: string } }
        >(`node:${guard.reference}`);
        if (
          node?.workspace !== request.scope.context.workspace_ref ||
          node.record.revision !== guard.expected_revision
        ) throw new Error("source authority revision conflict");
      }
      let journalReceipt;
      if (request.journal) {
        const nested = request.journal.request;
        if (
          nested.context.workspace_ref !==
            request.scope.context.workspace_ref ||
          nested.request_id === request.request_id
        ) throw new Error("invalid nested execution event identity");
        if (
          await tx.get(
            await receiptKey(
              nested.context.workspace_ref,
              nested.context.caller_ref,
              nested.request_id,
            ),
          )
        ) {
          throw new Error(
            "execution event already committed; reconcile and reload CWR state",
          );
        }
        const inTransaction: Storage = {
          get: tx.get.bind(tx),
          put: tx.put.bind(tx),
          delete: tx.delete.bind(tx),
          transaction: (work) => work(tx),
        };
        const result = await new Invocations(inTransaction, this.authorize)
          .mutate(nested, request.journal.now_unix_ms);
        if (result.outcome !== "committed") {
          throw new Error("execution result expired");
        }
        journalReceipt = result.receipt;
      }
      const next = counter + 1;
      if (!Number.isSafeInteger(next)) {
        throw new Error("CWR commit revision exhausted");
      }
      const writer = new Writer(graph, request.scope, next);
      let changed = false;
      for (const record of changes.records) {
        changed = await writer.record(record) || changed;
      }
      for (const record of changes.remove_records) {
        if (record.kind === "runtime") {
          throw new Error("runtime identity cannot be removed");
        }
        const old = await graph.get(record.reference);
        if (old && old.kind !== record.kind) {
          throw new Error("CWR record kind conflict");
        }
        changed = await graph.remove(record.reference) || changed;
      }
      for (const membership of changes.memberships) {
        changed = await writer.membership(membership) || changed;
      }
      for (const item of changes.timeline_items ?? []) {
        changed = await writer.record(timelineRecord(item)) || changed;
      }
      for (const item of changes.timeline_items ?? []) {
        for (const membership of timelineMemberships(item)) {
          changed = await writer.membership(membership) || changed;
        }
      }
      for (const publication of changes.publications) {
        changed = await publish(writer, publication) || changed;
      }
      if (changes.state_version !== null) {
        if (![STATE, TIMELINE_STATE].includes(changes.state_version)) {
          throw new Error("unsupported CWR runtime state version");
        }
        changed = await writer.metadata(
          "runtime_state_version",
          changes.state_version,
        ) || changed;
      }
      if (changes.streaming_runs !== null) {
        changed =
          await writer.metadata("streaming_runs", changes.streaming_runs) ||
          changed;
      }
      if (changes.timeline_rollout != null) {
        changed = await writer.rollout(changes.timeline_rollout) || changed;
      }
      if (changes.context_policy != null) {
        if (
          typeof changes.context_policy !== "object" ||
          Array.isArray(changes.context_policy)
        ) throw new Error("invalid retained context policy wrapper");
        changed =
          await writer.metadata("context_policy", changes.context_policy) ||
          changed;
      }
      if (changes.repair_legacy) {
        changed = await repair(writer, request) || changed;
      }
      if (changed) await writer.metadata("commit_revision", next);
      const outcome: Outcome = {
        outcome: "committed",
        revision: changed
          ? await digest([rootRef(request.scope), next])
          : revision,
        commit_revision: changed ? next : counter,
        ...(journalReceipt ? { journal_receipt: journalReceipt } : {}),
      };
      const key = await receiptKey(
        request.scope.context.workspace_ref,
        request.scope.context.caller_ref,
        request.request_id,
      );
      await tx.put<SavedReceipt>(key, {
        digest: await digest(request),
        outcome,
      });
      for (const reference of dependencies(request)) {
        await addDependency(tx, reference, key);
      }
      return outcome;
    });
  }
}
function normalize(input: Mutation): Mutation {
  validContext(input.scope.context, input.scope.runtime_root_ref);
  if (!input.request_id || input.request_id.length > 128) {
    throw new Error("invalid mutation request ID");
  }
  return {
    ...input,
    journal: input.journal ?? null,
    authority_guards: input.authority_guards ?? [],
    changes: {
      ...input.changes,
      repair_legacy: input.changes.repair_legacy ?? false,
      timeline_rollout: input.changes.timeline_rollout ?? null,
      context_policy: input.changes.context_policy ?? null,
      publications: input.changes.publications.map((p) => ({
        ...p,
        activities: p.activities ?? [],
        remove_activities: p.remove_activities ?? [],
      })),
    },
  };
}
async function receiptKey(
  workspace: string,
  caller: string,
  request: string,
): Promise<string> {
  return `receipt:${await digest([workspace, caller, request])}`;
}
async function reconcile(
  tx: Transaction,
  request: Mutation,
): Promise<Outcome | null> {
  const saved = await tx.get<SavedReceipt>(
    await receiptKey(
      request.scope.context.workspace_ref,
      request.scope.context.caller_ref,
      request.request_id,
    ),
  );
  if (!saved) return null;
  if (!saved.outcome) return { outcome: "committed_result_expired" };
  if (saved.digest !== await digest(request)) {
    throw new Error("mutation request conflict");
  }
  return saved.outcome;
}
function dependencies(request: Mutation): Set<string> {
  const refs = new Set(
    request.changes.records.map((record) => record.reference),
  );
  for (const item of request.changes.timeline_items ?? []) {
    refs.add(item.item_ref);
    refs.add(item.conversation_ref);
    refs.add(item.connection_ref);
    refs.add(item.publication_ref);
    refs.add(
      item.content.kind === "message"
        ? item.content.message_ref
        : item.content.invocation_ref,
    );
    if (item.child_connection_ref) refs.add(item.child_connection_ref);
  }
  for (const publication of request.changes.publications) {
    refs.add(publication.conversation_ref);
  }
  for (const change of request.changes.memberships) {
    if (change.operation === "replace") {
      refs.add(change.membership.source_ref);
      for (const member of change.membership.members) {
        refs.add(member.target_ref);
      }
    } else {
      refs.add(change.source_ref);
      refs.add(
        change.operation === "upsert"
          ? change.member.target_ref
          : change.target_ref,
      );
    }
  }
  return refs;
}
