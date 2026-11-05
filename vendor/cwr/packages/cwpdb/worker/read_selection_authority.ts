//! Authority is checked on declared domain identities before and after facts are selected.
import type { Authorizer } from "./engine.ts";
import type { ReadSelectionRequest } from "./read_selection_types.ts";
export async function authorizeSelection(
  request: ReadSelectionRequest,
  authorize: Authorizer,
): Promise<void> {
  const context = request.scope.context;
  await authorize(context, request.scope.runtime_root_ref, false);
  const op = request.selection;
  const allow = async (value: unknown) => {
    if (typeof value === "string") await authorize(context, value, false);
  };
  // Only the closed, validated operation fields participate. Cursor payloads,
  // text selectors and opaque work-activity IDs are not resource identities.
  for (const [key, value] of Object.entries(op)) {
    if (
      (key.endsWith("_ref") || key === "reference") &&
      !(op.operation === "latest_work_activity" && key === "activity_ref")
    ) await allow(value);
  }
  if (op.operation === "inspector_page" || op.operation === "inspector_text") {
    await allow(op.request.conversation_ref);
    await allow(op.request.invocation_ref);
    await allow(op.request.actor_ref);
  }
  if (op.operation === "summary_window") {
    await allow(op.request.participant_actor_ref);
  }
}
export async function authorizeSelectionResult(
  request: ReadSelectionRequest,
  result: unknown,
  authorize: Authorizer,
): Promise<void> {
  if (
    request.selection.operation === "resource" && result &&
    typeof result === "object" && !Array.isArray(result)
  ) {
    const conversation = (result as Record<string, unknown>).conversation_ref;
    if (typeof conversation === "string") {
      await authorize(request.scope.context, conversation, false);
    }
  }
  if (
    (request.selection.operation === "inspector_invocation_root" ||
      request.selection.operation === "timeline_item_conversation") &&
    typeof result === "string"
  ) await authorize(request.scope.context, result, false);
}
