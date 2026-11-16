import {
  canonical,
  digest,
  type Storage,
  type Transaction,
  validContext,
} from "./engine.ts";
import type { Context } from "./client.ts";
import { bindIdentity } from "./identity.ts";
import { Graph } from "./conversation_graph.ts";
import type {
  Advance,
  Installed,
  Occurrence,
  ScheduleRequest,
  Versioned,
} from "./schedule_types.ts";
interface Saved<T> extends Versioned<T> {
  root: string;
}
const k = (...parts: unknown[]) => `schedule:${canonical(parts)}`;
const identity = async (kind: string, value: unknown) =>
  `cwp://org.coworkerprotocol.local/${kind}/${await digest(value)}`;
function owner(value: Installed, expected?: string | null): void {
  if (
    expected !== null && expected !== undefined &&
    value.owner_context_ref !== expected
  ) throw new Error("schedule is unavailable in this context");
}
function expected(actual: number, revision: number): void {
  if (actual !== revision) {
    throw new Error("schedule revision conflict; reload owning state");
  }
}
function time(value: number): void {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error("schedule time exceeds safe millisecond range");
  }
}
async function get<T>(
  tx: Transaction,
  kind: string,
  reference: string,
): Promise<Saved<T> | null> {
  return await tx.get<Saved<T>>(k(kind, reference)) ?? null;
}
async function write<T>(
  tx: Transaction,
  kind: string,
  reference: string,
  root: string,
  previous: Saved<T> | null,
  value: T,
): Promise<Versioned<T>> {
  const revision = previous ? previous.revision + 1 : 0;
  if (!Number.isSafeInteger(revision)) {
    throw new Error("schedule revision exhausted");
  }
  await bindIdentity(
    tx,
    reference,
    `node:${kind}`,
    root,
    `cwp://org.coworkerprotocol/cwpdb/schemas/${kind}/2026-09`,
  );
  await tx.put(k(kind, reference), { value, revision, root });
  return { value, revision };
}
async function add(
  tx: Transaction,
  index: string,
  reference: string,
): Promise<void> {
  const refs = await tx.get<string[]>(index) ?? [];
  if (refs.includes(reference)) return;
  if (refs.length >= 20000) {
    throw new Error("schedule index work limit exceeded");
  }
  await tx.put(index, [...refs, reference]);
}
async function records<T>(
  tx: Transaction,
  index: string,
  kind: string,
): Promise<Versioned<T>[]> {
  const result = [];
  for (const ref of await tx.get<string[]>(index) ?? []) {
    const record = await get<T>(tx, kind, ref);
    if (!record) throw new Error("schedule index record missing");
    result.push({ value: record.value, revision: record.revision });
  }
  return result;
}
async function installed(
  tx: Transaction,
  root: string,
  scope?: string | null,
): Promise<Versioned<Installed>[]> {
  const owners = scope !== undefined && scope !== null
    ? [scope]
    : await tx.get<string[]>(k("owners", root)) ?? [];
  const result: Versioned<Installed>[] = [];
  for (const value of owners) {
    result.push(
      ...await records<Installed>(
        tx,
        k("installed", value),
        "installed_schedule",
      ),
    );
  }
  for (const record of result) owner(record.value, scope);
  return result;
}
async function advance(
  tx: Transaction,
  request: Advance,
): Promise<Versioned<Installed>> {
  time(request.next_due_unix_ms);
  const old = await get<Installed>(
    tx,
    "installed_schedule",
    request.schedule_ref,
  );
  if (!old) throw new Error("missing schedule");
  expected(old.revision, request.expected);
  if (request.active_occurrence_ref) {
    const occurrence = await get<Occurrence>(
      tx,
      "schedule_occurrence",
      request.active_occurrence_ref,
    );
    if (!occurrence || occurrence.value.schedule_ref !== request.schedule_ref) {
      throw new Error("active occurrence mismatch");
    }
  }
  return await write(
    tx,
    "installed_schedule",
    request.schedule_ref,
    old.root,
    old,
    {
      ...old.value,
      next_due_unix_ms: request.next_due_unix_ms,
      active_occurrence_ref: request.active_occurrence_ref,
      last_admission_error: request.last_admission_error,
    },
  );
}
export class Schedules {
  constructor(private readonly storage: Storage) {}
  async dispatch(context: Context, input: ScheduleRequest): Promise<unknown> {
    validContext(context, context.workspace_ref);
    if (new TextEncoder().encode(canonical(input)).length > 512 * 1024) {
      throw new Error("schedule request exceeds512KiB");
    }
    return await this.storage.transaction(async (tx) => {
      const result = await dispatch(tx, context, input);
      if (
        new TextEncoder().encode(canonical(result)).length > 24 * 1024 * 1024
      ) throw new Error("schedule response exceeds24MiB");
      return result;
    });
  }
}
async function dispatch(
  tx: Transaction,
  context: Context,
  input: ScheduleRequest,
): Promise<unknown> {
  if (input.action === "installed" || input.action === "archive_candidates") {
    validContext(context, input.runtime_root_ref);
    const schedules = await installed(tx, input.runtime_root_ref, input.owner);
    if (input.action === "installed") return schedules;
    const result: [string, Versioned<Occurrence>][] = [];
    for (const schedule of schedules) {
      for (
        const occurrence of await records<Occurrence>(
          tx,
          k("terminal", schedule.value.schedule_ref),
          "schedule_occurrence",
        )
      ) result.push([schedule.value.author_actor_ref, occurrence]);
    }
    return result;
  }
  if (input.action === "get_schedule" || input.action === "get_occurrence") {
    validContext(context, input.reference);
    if (input.action === "get_schedule") {
      const record = await get<Installed>(
        tx,
        "installed_schedule",
        input.reference,
      );
      if (record) owner(record.value, input.owner);
      return record ? { value: record.value, revision: record.revision } : null;
    }
    const record = await get<Occurrence>(
      tx,
      "schedule_occurrence",
      input.reference,
    );
    return record ? { value: record.value, revision: record.revision } : null;
  }
  if (input.action === "occurrences") {
    if (input.selection === "UnarchivedTerminal") {
      return await records<Occurrence>(
        tx,
        k("terminal", input.reference),
        "schedule_occurrence",
      );
    }
    const limit = input.selection.Recent;
    if (!Number.isInteger(limit) || limit < 1 || limit > 100) {
      throw new Error("recent run limit must be1to100");
    }
    return (await records<Occurrence>(
      tx,
      k("recent", input.reference),
      "schedule_occurrence",
    )).slice(0, limit);
  }
  if (input.action === "install") {
    const r = input.request;
    validContext(context, r.runtime_root_ref);
    validContext(context, r.author_actor_ref);
    time(r.next_due_unix_ms);
    if (!r.owner_context_ref.trim() || r.owner_context_ref.length > 2048) {
      throw new Error("installed owner context is required");
    }
    if (typeof r.definition.declaration_ref !== "string") {
      throw new Error("schedule declaration ref required");
    }
    const reference = await identity("installed-schedules", [
      r.owner_context_ref,
      r.definition.declaration_ref,
    ]);
    const old = await get<Installed>(tx, "installed_schedule", reference);
    let value: Installed = old ? { ...old.value } : {
      schedule_ref: reference,
      owner_context_ref: r.owner_context_ref,
      author_actor_ref: r.author_actor_ref,
      definition: r.definition,
      availability: "ready",
      paused: false,
      next_due_unix_ms: r.next_due_unix_ms,
      active_occurrence_ref: null,
      last_admission_error: null,
    };
    owner(value, r.owner_context_ref);
    if (canonical(value.definition) !== canonical(r.definition)) {
      if (
        canonical(value.definition.revision) ===
          canonical(r.definition.revision)
      ) throw new Error("schedule revision was reused with different content");
      value = {
        ...value,
        definition: r.definition,
        next_due_unix_ms: value.active_occurrence_ref
          ? value.next_due_unix_ms
          : r.next_due_unix_ms,
      };
    }
    const graph = new Graph(tx, {
      context,
      runtime_root_ref: r.runtime_root_ref,
    });
    if (!(await graph.get(`${r.runtime_root_ref}/cwpdb`))) {
      throw new Error("schedule runtime is unavailable");
    }
    const ownerRef = await identity(
      "installed-schedule-owners",
      r.owner_context_ref,
    );
    await bindIdentity(
      tx,
      ownerRef,
      "node:schedule_owner",
      r.runtime_root_ref,
      "cwp://org.coworkerprotocol/cwpdb/schemas/schedule_owner/2026-09",
    );
    await add(tx, k("owners", r.runtime_root_ref), r.owner_context_ref);
    await add(tx, k("installed", r.owner_context_ref), reference);
    return await write(
      tx,
      "installed_schedule",
      reference,
      r.runtime_root_ref,
      old,
      value,
    );
  }
  if (input.action === "pause") {
    time(input.next_due_unix_ms);
    const old = await get<Installed>(tx, "installed_schedule", input.reference);
    if (!old) throw new Error("schedule is unavailable in this context");
    owner(old.value, input.owner);
    expected(old.revision, input.expected);
    const value = { ...old.value };
    if (value.paused === input.paused) {
      return { value: old.value, revision: old.revision };
    }
    if (value.paused !== input.paused) {
      value.paused = input.paused;
      if (!input.paused) value.next_due_unix_ms = input.next_due_unix_ms;
    }
    return await write(
      tx,
      "installed_schedule",
      input.reference,
      old.root,
      old,
      value,
    );
  }
  if (input.action === "availability") {
    for (const change of input.changes) {
      if (
        !["ready", "disabled", "needs_setup", "removed"].includes(
          change.availability,
        )
      ) throw new Error("unsupported installed schedule availability");
      time(change.next_due_unix_ms);
      const old = await get<Installed>(
        tx,
        "installed_schedule",
        change.schedule_ref,
      );
      if (!old) throw new Error("missing schedule");
      owner(old.value, input.owner);
      expected(old.revision, change.expected);
      if (old.value.availability !== change.availability) {
        await write(
          tx,
          "installed_schedule",
          change.schedule_ref,
          old.root,
          old,
          {
            ...old.value,
            availability: change.availability,
            next_due_unix_ms: change.availability === "ready"
              ? change.next_due_unix_ms
              : old.value.next_due_unix_ms,
          },
        );
      }
    }
    return null;
  }
  if (input.action === "advance") return await advance(tx, input.request);
  if (input.action === "reserve") {
    const r = input.request;
    const o = r.occurrence;
    time(o.due_unix_ms);
    const schedule = await get<Installed>(
      tx,
      "installed_schedule",
      r.schedule_ref,
    );
    if (!schedule) throw new Error("missing schedule");
    expected(schedule.revision, r.expected);
    if (
      o.schedule_ref !== r.schedule_ref ||
      canonical(o.definition) !== canonical(schedule.value.definition) ||
      o.occurrence_ref !==
        await identity("schedule-occurrences", [
          r.schedule_ref,
          o.definition.revision,
          o.due_unix_ms,
        ])
    ) throw new Error("schedule occurrence identity mismatch");
    if (o.invocation_ref || o.conversation_ref || o.archived) {
      throw new Error("new occurrence cannot attach execution or archival");
    }
    const old = await get<Occurrence>(
      tx,
      "schedule_occurrence",
      o.occurrence_ref,
    );
    if (old && canonical(old.value) !== canonical(o)) {
      throw new Error("schedule occurrence identity was reused");
    }
    await write(
      tx,
      "schedule_occurrence",
      o.occurrence_ref,
      schedule.root,
      old,
      o,
    );
    if (!old) {
      const ordinal = (await tx.get<number>(k("order")) ?? 0) + 1;
      if (!Number.isSafeInteger(ordinal)) {
        throw new Error("schedule retained order exhausted");
      }
      await tx.put(k("order"), ordinal);
      await tx.put(k("ordinal", o.occurrence_ref), ordinal);
    }

    const recent = await tx.get<string[]>(k("recent", r.schedule_ref)) ?? [];
    const refs = [...new Set([...recent, o.occurrence_ref])];
    const values = await Promise.all(
      refs.map((ref) => get<Occurrence>(tx, "schedule_occurrence", ref)),
    );
    const ordinals = new Map<string, number>();
    for (const ref of refs) {
      ordinals.set(ref, (await tx.get<number>(k("ordinal", ref)))!);
    }
    values.sort((a, b) =>
      b!.value.due_unix_ms - a!.value.due_unix_ms ||
      ordinals.get(b!.value.occurrence_ref)! -
        ordinals.get(a!.value.occurrence_ref)!
    );
    await tx.put(
      k("recent", r.schedule_ref),
      values.slice(0, 100).map((v) => v!.value.occurrence_ref),
    );
    if (o.outcome.startsWith("terminal_")) {
      await add(tx, k("terminal", r.schedule_ref), o.occurrence_ref);
    }
    return await advance(tx, {
      ...r,
      last_admission_error: schedule.value.last_admission_error,
    });
  }
  if (input.action === "settle") {
    const r = input.request;
    const old = await get<Occurrence>(
      tx,
      "schedule_occurrence",
      r.occurrence_ref,
    );
    if (!old) throw new Error("missing occurrence");
    expected(old.revision, r.expected);
    if (
      old.value.invocation_ref &&
        old.value.invocation_ref !== r.invocation_ref ||
      old.value.conversation_ref &&
        old.value.conversation_ref !== r.conversation_ref
    ) throw new Error("retained schedule execution identity changed");
    if ((r.invocation_ref === null) !== (r.conversation_ref === null)) {
      throw new Error(
        "schedule execution requires invocation and conversation",
      );
    }
    if (
      old.value.archived && !r.archived ||
      old.value.outcome.startsWith("terminal_") &&
        !r.outcome.startsWith("terminal_") ||
      r.archived && !r.outcome.startsWith("terminal_")
    ) throw new Error("schedule terminal/archival state cannot regress");
    if (r.invocation_ref && r.conversation_ref) {
      const graph = new Graph(tx, { context, runtime_root_ref: old.root });
      if (
        (await graph.get(r.invocation_ref))?.kind !== "invocation" ||
        (await graph.get(r.conversation_ref))?.kind !== "conversation"
      ) throw new Error("schedule execution endpoint is unavailable");
    }
    const value = {
      ...old.value,
      outcome: r.outcome,
      invocation_ref: r.invocation_ref,
      conversation_ref: r.conversation_ref,
      archived: r.archived,
    };
    const terminal = k("terminal", value.schedule_ref);
    if (value.outcome.startsWith("terminal_") && !value.archived) {
      await add(tx, terminal, r.occurrence_ref);
    } else {await tx.put(
        terminal,
        (await tx.get<string[]>(terminal) ?? []).filter((ref) =>
          ref !== r.occurrence_ref
        ),
      );}
    return await write(
      tx,
      "schedule_occurrence",
      r.occurrence_ref,
      old.root,
      old,
      value,
    );
  }
  throw new Error("unsupported schedule operation");
}
