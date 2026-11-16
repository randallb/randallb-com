import {
  authorizeSelection,
  authorizeSelectionResult,
} from "./read_selection_authority.ts";
import {
  selectionCompletionPage,
  selectionContentMessage,
  selectionContentPositions,
  selectionLegacyTimelineMessage,
  selectionLegacyTimelinePage,
  selectionTimelinePage,
} from "./read_selection_pages.ts";
import { SelectionSummary } from "./read_selection_summary.ts";
import type { Authorizer } from "./engine.ts";
import { Graph, rootRef } from "./conversation_graph.ts";
import { historyRead } from "./conversation_history_read.ts";
import { integerOrNull } from "./conversation_resource.ts";
import { MAX_WIRE_BYTES, object } from "./conversation_types.ts";
import { SelectionReader } from "./read_selection_reader.ts";
import { SelectionTimeline } from "./read_selection_timeline.ts";
import { SelectionInspector } from "./read_selection_inspector.ts";
import type {
  ReadSelectionFacts,
  ReadSelectionRequest,
} from "./read_selection_types.ts";
import { validateSelection } from "./read_selection_validate.ts";

export async function readSelection(
  graph: Graph,
  request: ReadSelectionRequest,
  authorize: Authorizer,
): Promise<ReadSelectionFacts> {
  if (
    new TextEncoder().encode(JSON.stringify(request)).length >
      MAX_WIRE_BYTES + 65536
  ) throw new Error("declared read exceeds byte budget");
  validateSelection(request);
  await authorizeSelection(request, authorize);
  if (
    !Number.isSafeInteger(request.max_bytes) || request.max_bytes < 1 ||
    request.max_bytes > MAX_WIRE_BYTES
  ) throw new Error("invalid read selection byte budget");
  if (
    !Number.isSafeInteger(request.deadline_unix_ms) ||
    Date.now() >= request.deadline_unix_ms
  ) throw new Error("conversation read operation exceeded its deadline");
  if (!await graph.get(rootRef(graph.scope))) {
    throw new Error("conversation runtime is not initialized");
  }
  const root = await graph.ensure(),
    counter = integerOrNull(root.properties.commit_revision);
  if (counter === null || counter < 0) {
    throw new Error("invalid CWR commit revision");
  }
  const revision = String(counter);
  if (
    (request.selection.operation !== "position" &&
      request.expected_commit_revision == null) ||
    (request.expected_commit_revision != null &&
      request.expected_commit_revision !== revision)
  ) throw new Error("conversation read snapshot revision conflict");
  const reader = new SelectionReader(graph, request.deadline_unix_ms);
  const result = await dispatch(reader, request, authorize);
  const after = integerOrNull(
    (await graph.ensure()).properties.commit_revision,
  );
  if (after !== counter) {
    throw new Error("conversation read snapshot revision conflict");
  }
  if (Date.now() >= request.deadline_unix_ms) {
    throw new Error("conversation read operation exceeded its deadline");
  }
  await authorizeSelection(request, authorize);
  await authorizeSelectionResult(request, result, authorize);
  const facts = {
    runtime_root_ref: graph.scope.runtime_root_ref,
    commit_revision: revision,
    result,
  };
  if (
    new TextEncoder().encode(JSON.stringify(facts)).length > request.max_bytes
  ) throw new Error("read selection exceeds response byte budget");
  return facts;
}
async function dispatch(
  reader: SelectionReader,
  request: ReadSelectionRequest,
  authorize: Authorizer,
): Promise<unknown> {
  const op = request.selection,
    timeline = new SelectionTimeline(reader),
    inspector = new SelectionInspector(reader, authorize);
  switch (op.operation) {
    case "position":
      return null;
    case "content_message":
      return await selectionContentMessage(
        reader,
        op.conversation_ref,
        op.message_ref,
        op.include_execution,
        request,
      );
    case "content_positions":
      return await selectionContentPositions(
        reader,
        op.conversation_ref,
        op.window,
        op.include_execution,
        request,
      );
    case "completion_page":
      return await selectionCompletionPage(
        reader,
        op.conversation_ref,
        op.window,
        op.include_execution,
        request,
      );
    case "timeline_page":
      return await selectionTimelinePage(
        reader,
        op.conversation_ref,
        op.window,
        op.include_execution,
        request,
      );
    case "legacy_timeline_page":
      return await selectionLegacyTimelinePage(
        reader,
        op.conversation_ref,
        op.window,
        op.include_execution,
        request,
      );
    case "legacy_timeline_message":
      return await selectionLegacyTimelineMessage(
        reader,
        op.conversation_ref,
        op.key,
        op.message_ref,
        op.include_execution,
        request,
      );

    case "summary_window":
      return await new SelectionSummary(reader).window(op.request);
    case "summary_row":
      return await new SelectionSummary(reader).one(op.reference, op.workspace);
    case "summary_message_record": {
      const record = await reader.message(op.conversation_ref, op.message_ref);
      if (!record || record.message_ref !== op.message_ref) {
        throw new Error("conversation summary message identity mismatch");
      }
      return record;
    }
    case "history_read":
      return await historyRead(reader.graph, {
        scope: request.scope,
        conversation_ref: op.conversation_ref,
        selection: op.selection,
        participant_actor_ref: op.participant_actor_ref,
        include_execution: op.include_execution,
        expected_commit_revision: request.expected_commit_revision,
        deadline_unix_ms: request.deadline_unix_ms,
        max_bytes: request.max_bytes,
      });
    case "current":
      return await reader.current(op.conversation_ref);
    case "invocation":
      return await reader.invocation(op.conversation_ref, op.invocation_ref);
    case "resource":
      return await reader.facts(op.reference, authorize);
    case "conversation_exists":
      return await reader.conversationExists(
        op.conversation_ref,
        op.include_execution,
        op.owner,
      );
    case "participant_visible":
      return await reader.participantVisible(op.conversation_ref, op.actor_ref);
    case "root_conversation_presentation":
      return await reader.rootPresentation(op.conversation_ref);
    case "fork_source":
      return await reader.forkSource(op.conversation_ref);
    case "owner_context": {
      const header = await reader.header(op.conversation_ref);
      if (!header) throw new Error("conversation unavailable");
      return typeof header.node.properties.owner_context_ref === "string"
        ? header.node.properties.owner_context_ref
        : null;
    }
    case "timeline_item_conversation":
      return await reader.itemConversation(op.item_ref);
    case "actor_identity":
      return await reader.actorIdentity(op.actor_ref);
    case "actor_candidate":
      return await reader.actorCandidate(op.actor_ref);
    case "actor_conversations":
      return await reader.actorConversations(op.actor_ref);
    case "actor_resources":
      return await reader.actorResources(op.actor_ref);
    case "artifact_exists":
      return await reader.artifactExists(op.reference);
    case "artifact_messages":
      return await reader.artifactMessages(op.reference);
    case "artifact_html":
      return await reader.artifactHtml(op.reference);
    case "invocation_reply_ref":
      return await reader.replyRef(op.invocation_ref);
    case "invocation_fork_destination":
      return await reader.forkDestination(
        op.invocation_ref,
        op.intrinsic_actor_ref,
      );
    case "invocation_stop_facts":
      return await reader.stopFacts(op.invocation_ref, op.intrinsic_actor_ref);
    case "invocation_visible":
      return await reader.visible(
        op.root_ref,
        op.invocation_ref,
        "conversation_timeline_invocation",
      );
    case "streaming_message_metadata": {
      const node = await reader.node(op.message_ref);
      const stream = node && object(node.properties.streaming);
      if (!stream || typeof stream.invocation_ref !== "string") {
        throw new Error("streaming invocation is unavailable");
      }
      return [
        typeof stream.phase === "string" ? stream.phase : null,
        stream.invocation_ref,
      ];
    }
    case "message_action_invocation":
      return await reader.actionInvocation(op.conversation_ref, op.message_ref);
    case "progress_activity":
      return await reader.progressActivity(
        op.invocation_ref,
        op.source_activity_ref,
      );
    case "progress_in_latest_attempt":
      return await reader.progressInLatestAttempt(
        op.invocation_ref,
        op.sequence,
      );
    case "latest_work_activity":
      return await reader.latestWorkActivity(
        op.invocation_ref,
        op.activity_ref,
      );
    case "timeline_root":
      return await timeline.root(op.conversation_ref, op.include_execution);
    case "timeline_connection_metadata":
      return await timeline.metadata(
        op.conversation_ref,
        op.root_ref,
        op.connection_ref,
      );
    case "timeline_cursor_exists":
      return await timeline.cursorExists(
        op.connection_ref,
        op.sequence,
        op.item_ref,
      );
    case "timeline_connection_items":
      return await timeline.items(
        op.connection_ref,
        op.lower,
        op.upper,
        op.limit,
        op.backwards,
      );
    case "timeline_connection_count":
      return await timeline.count(op.conversation_ref, op.connection_ref);
    case "timeline_item":
      return await timeline.item(op.conversation_ref, op.item_ref);
    case "inspector_capture_available":
      return await inspector.capture(op.invocation_ref);
    case "inspector_details_allowed":
      return await inspector.details(op.invocation_ref, op.actor_ref);
    case "inspector_invocation_root":
      return await inspector.root(op.invocation_ref);
    case "inspector_scope":
      return await inspector.scope(
        op.actor_ref,
        op.invocation_ref,
        op.collection,
        op.activity_ref,
      );
    case "inspector_page":
      return await inspector.page(op.request);
    case "inspector_activity_sequence":
      return await inspector.sequence(op.invocation_ref, op.activity_ref);
    case "inspector_exit_code":
      return await inspector.exitCode(
        op.invocation_ref,
        op.conversation_ref,
        op.actor_ref,
        op.activity_ref,
      );
    case "inspector_text":
      return await inspector.text(op.request);
    case "invocation_target_source":
      return await reader.targetSource(op.invocation_ref);
    case "message_is_commentary":
      return await reader.isCommentary(op.message_ref, op.invocation_ref);
    case "message_is_final_reply":
      return await reader.isFinalReply(
        op.message_ref,
        op.target_actor_ref,
        op.source_message_ref,
      );
    case "last_addressed_actors":
      return await timeline.lastAddressed(
        op.conversation_ref,
        op.root_ref,
        op.author_actor_ref,
      );
  }
}
