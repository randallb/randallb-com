import type { ReadSelectionRequest } from "./read_selection_types.ts";
/// <reference path="./runtime.d.ts" />
import type { HistoryReadRequest } from "./conversation_history_read.ts";
import type { TimelineReadRequest } from "./conversation_timeline_read.ts";
import type {
  CompletionPageRequest,
  ContentPageRequest,
} from "./conversation_page_types.ts";
import { DurableObject } from "cloudflare:workers";
import type { Context, Mutation } from "./client.ts";
import { digest, Documents } from "./engine.ts";
import { DurableKv } from "./kv.ts";
import { Schedules } from "./schedules.ts";
import type { ScheduleRequest } from "./schedule_types.ts";
import { Conversations } from "./conversations.ts";
import type {
  Mutation as ConversationMutation,
  ReadRequest,
  Scope,
} from "./conversation_types.ts";
import {
  Invocations,
  type Mutation as InvocationMutation,
} from "./invocations.ts";
interface Env {
  CWPDB_COORDINATOR_TOKEN: string;
}
/** This Durable Object is reachable only through a namespace/service binding.
 * The ordinary Worker HTTP entrypoint always denies. The trusted coordinator
 * resolves current actor/workspace/schema authority before invoking this port;
 * executor credentials and caller-submitted context cannot access it. */
export class WorkspaceStorage extends DurableObject<Env> {
  private readonly storage = new DurableKv(this.ctx.storage);
  private async authorized(token: string): Promise<void> {
    if (
      !this.env.CWPDB_COORDINATOR_TOKEN ||
      this.env.CWPDB_COORDINATOR_TOKEN.length < 32 ||
      await digest(token) !== await digest(this.env.CWPDB_COORDINATOR_TOKEN)
    ) throw new Error("storage access denied");
  }
  async dispatch(
    token: string,
    body: {
      operation: string;
      source_ref?: string;
      target_ref?: string;
      membership_kind?: string;
      schedule_request?: ScheduleRequest;
      context?: Context;
      reference?: string;
      request?:
        | Mutation
        | ConversationMutation
        | ReadRequest
        | ContentPageRequest
        | CompletionPageRequest
        | TimelineReadRequest
        | HistoryReadRequest
        | ReadSelectionRequest;
      scope?: Scope;
      invocation_request?: InvocationMutation;
      invocation_ref?: string;
      after?: number | null;
      limit?: number;
      now_unix_ms?: number;
      request_id?: string;
    },
  ): Promise<unknown> {
    await this.authorized(token);
    const context =
      (body.request && "scope" in body.request
        ? body.request.scope.context
        : body.request?.context) ??
        body.scope?.context ?? body.invocation_request?.context ?? body.context;
    if (!context) throw new Error("missing authorized context");
    await this.storage.transaction(async (tx) => {
      const pinned = await tx.get<string>("workspace_binding");
      if (pinned && pinned !== context.workspace_ref) {
        throw new Error("storage workspace mismatch");
      }
      if (!pinned) await tx.put("workspace_binding", context.workspace_ref);
    });
    if (body.operation === "schedule" && body.schedule_request) {
      return await new Schedules(this.storage).dispatch(
        context,
        body.schedule_request,
      );
    }
    const documents = new Documents(this.storage, async (c) => {
      if (c.workspace_ref !== context.workspace_ref) {
        throw new Error("storage workspace mismatch");
      }
    }, async () => {});
    if (body.operation === "get_document" && body.reference) {
      return await documents.get(context, body.reference);
    }
    if (body.operation === "reconcile_document" && body.request) {
      return await documents.reconcile(body.request as Mutation);
    }
    if (body.operation === "mutate_document" && body.request) {
      return await documents.mutate(body.request as Mutation);
    }
    const invocations = new Invocations(this.storage, async (c) => {
      if (c.workspace_ref !== context.workspace_ref) {
        throw new Error("storage workspace mismatch");
      }
    });
    if (
      body.operation === "mutate_invocation" && body.invocation_request &&
      body.now_unix_ms !== undefined
    ) {
      return await invocations.mutate(
        body.invocation_request,
        body.now_unix_ms,
      );
    }
    if (
      body.operation === "invocation_receipt" && body.invocation_ref &&
      body.request_id
    ) {
      return await invocations.receipt(
        context,
        body.invocation_ref,
        body.request_id,
      );
    }
    if (body.operation === "get_invocation" && body.invocation_ref) {
      return await invocations.get(context, body.invocation_ref);
    }
    if (
      body.operation === "invocation_events" && body.invocation_ref &&
      body.limit !== undefined
    ) {
      return await invocations.events(
        context,
        body.invocation_ref,
        body.after ?? null,
        body.limit,
      );
    }
    const conversations = new Conversations(this.storage, async (c) => {
      if (c.workspace_ref !== context.workspace_ref) {
        throw new Error("storage workspace mismatch");
      }
    });
    if (body.operation === "read_conversation_selection" && body.request) {
      return await conversations.readSelection(
        body.request as ReadSelectionRequest,
      );
    }
    if (body.operation === "conversation_history_read" && body.request) {
      return await conversations.historyRead(
        body.request as HistoryReadRequest,
      );
    }
    if (body.operation === "conversation_timeline_page" && body.request) {
      return await conversations.timelinePage(
        body.request as TimelineReadRequest,
      );
    }
    if (body.operation === "conversation_content_page" && body.request) {
      return await conversations.contentPage(
        body.request as ContentPageRequest,
      );
    }
    if (body.operation === "conversation_completion_page" && body.request) {
      return await conversations.completionPage(
        body.request as CompletionPageRequest,
      );
    }
    if (
      body.operation === "conversation_resource" && body.scope && body.reference
    ) return await conversations.resource(body.scope, body.reference);
    if (
      body.operation === "conversation_endpoint_kinds" && body.scope &&
      body.reference
    ) {
      return await conversations.endpointKinds(body.scope, body.reference);
    }
    if (
      body.operation === "conversation_history_status" && body.scope &&
      body.reference
    ) return await conversations.historyStatus(body.scope, body.reference);
    if (
      body.operation === "conversation_contains_member" && body.scope &&
      body.source_ref && body.target_ref && body.membership_kind
    ) {
      return await conversations.containsMember(
        body.scope,
        body.source_ref,
        body.membership_kind,
        body.target_ref,
      );
    }
    if (
      body.operation === "conversation_summary" && body.scope && body.reference
    ) {
      return await conversations.summary(body.scope, body.reference);
    }
    if (
      body.operation === "conversation_identity_in_use" && body.scope &&
      body.reference
    ) {
      return await conversations.identityInUse(body.scope, body.reference);
    }
    if (
      body.operation === "conversation_retained_order" && body.scope &&
      body.reference
    ) return await conversations.retainedOrder(body.scope, body.reference);
    if (
      body.operation === "published_progress_conversation" && body.scope &&
      body.reference
    ) return await conversations.publishedProgress(body.scope, body.reference);
    if (body.operation === "read_conversation_graph" && body.request) {
      return await conversations.read(body.request as ReadRequest);
    }
    if (
      body.operation === "get_conversation_record" && body.scope &&
      body.reference
    ) return await conversations.get(body.scope, body.reference);
    if (body.operation === "commit_conversation_graph" && body.request) {
      return await conversations.commit(body.request as ConversationMutation);
    }
    if (body.operation === "reconcile_conversation_graph" && body.request) {
      return await conversations.reconcile(
        body.request as ConversationMutation,
      );
    }
    throw new Error("unsupported cwpdb operation");
  }
}
export default {
  fetch(): Response {
    return new Response("Not found\n", { status: 404 });
  },
};
