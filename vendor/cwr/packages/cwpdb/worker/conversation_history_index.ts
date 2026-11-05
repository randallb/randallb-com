/** Backend-owned current history flags; existing conversations get one bounded backfill. */
import { canonical } from "./engine.ts";
import type { Graph } from "./conversation_graph.ts";
import type { GraphRecord, Scope } from "./conversation_types.ts";
type Facet = "streaming" | "legacy" | "terminal";
interface Ready {
  version: 1;
  generation: string;
}
const facets: Facet[] = ["streaming", "legacy", "terminal"];
const k = (scope: Scope, ...parts: unknown[]) =>
  `cwr:${canonical([scope.runtime_root_ref, "history-v1", ...parts])}`;
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function legacy(value: unknown): boolean {
  if (!object(value)) return false;
  if (Object.hasOwn(value, "resumed_host_call")) return true;
  const pending: unknown[] = [value.thread];
  let count = 0;
  while (pending.length) {
    if (++count > 50000) {
      throw new Error("legacy context predicate work limit exceeded");
    }
    const next = pending.pop();
    if (Array.isArray(next)) pending.push(...next);
    else if (object(next)) {
      if (Object.hasOwn(next, "messages")) return true;
      pending.push(...Object.values(next));
    }
  }
  return false;
}
function values(record: GraphRecord | null): Record<Facet, boolean> {
  const props = record?.properties;
  const payload = props && object(props.record) ? props.record : {};
  const event = object(payload.event) ? payload.event : {};
  return {
    streaming: record?.kind === "message" && object(props?.streaming),
    legacy: record?.kind === "activity" && legacy(event.invocation),
    terminal: record?.kind === "activity" &&
      ["turn_completed", "turn_failed", "turn_stopped"].includes(
        String(event.kind),
      ),
  };
}
async function ready(graph: Graph, source: string): Promise<Ready | null> {
  const value = await graph.tx.get<Ready>(k(graph.scope, "ready", source));
  if (value && (value.version !== 1 || typeof value.generation !== "string")) {
    throw new Error("unsupported history flag index");
  }
  return value ?? null;
}
async function set(
  graph: Graph,
  source: string,
  target: string,
  facet: Facet,
  active: boolean,
): Promise<void> {
  const generation = (await ready(graph, source))?.generation ?? "pending";
  const marker = k(graph.scope, "marker", source, target, facet);
  const old = await graph.tx.get<{ generation: string; active: boolean }>(
    marker,
  );
  const previous = old?.generation === generation && old.active;
  if (Boolean(previous) === active) return;
  const countKey = k(graph.scope, "count", source, generation, facet);
  const count = (await graph.tx.get<number>(countKey) ?? 0) + (active ? 1 : -1);
  if (!Number.isSafeInteger(count) || count < 0) {
    throw new Error("invalid history flag count");
  }
  await graph.tx.put(marker, { generation, active });
  await graph.tx.put(countKey, count);
}
export async function historyMembership(
  graph: Graph,
  source: string,
  kind: string,
  target: string,
  present: boolean,
  known?: GraphRecord,
): Promise<void> {
  if (kind !== "message" && kind !== "activity") return;
  const flags = present
    ? values(known ?? await graph.get(target))
    : { streaming: false, legacy: false, terminal: false };
  for (
    const facet of kind === "message"
      ? ["streaming" as const]
      : ["legacy" as const, "terminal" as const]
  ) await set(graph, source, target, facet, flags[facet]);
}
export async function historyRecord(
  graph: Graph,
  record: GraphRecord,
): Promise<void> {
  if (record.kind !== "message" && record.kind !== "activity") return;
  const index = ["incident", record.reference];
  const end = await graph.count(index);
  if (end > 50000) throw new Error("history incident work limit exceeded");
  for (let slot = 1; slot <= end; slot++) {
    const tuple = await graph.entry(index, slot);
    if (!tuple) continue;
    const [source, kind, target] = JSON.parse(tuple) as string[];
    if (
      target !== record.reference || (kind !== "message" && kind !== "activity")
    ) continue;
    if (await graph.edge(source, kind, target)) {
      await historyMembership(graph, source, kind, target, true, record);
    }
  }
}
async function initialize(graph: Graph, source: string): Promise<Ready> {
  const existing = await ready(graph, source);
  if (existing) return existing;
  const generation = crypto.randomUUID(),
    counts: Record<Facet, number> = { streaming: 0, legacy: 0, terminal: 0 };
  let work = 0;
  for (const kind of ["message", "activity"]) {
    const index = ["edges", source, kind];
    for (let slot = 1, end = await graph.count(index); slot <= end; slot++) {
      if (++work > 50000) {
        throw new Error("history index backfill work limit exceeded");
      }
      const target = await graph.entry(index, slot);
      if (!target || !await graph.edge(source, kind, target)) continue;
      const flags = values(await graph.get(target));
      for (
        const facet of kind === "message"
          ? ["streaming" as const]
          : ["legacy" as const, "terminal" as const]
      ) {
        await graph.tx.put(k(graph.scope, "marker", source, target, facet), {
          generation,
          active: flags[facet],
        });
        if (flags[facet]) counts[facet]++;
      }
    }
  }
  for (const facet of facets) {
    await graph.tx.put(
      k(graph.scope, "count", source, generation, facet),
      counts[facet],
    );
  }
  const current: Ready = { version: 1, generation };
  await graph.tx.put(k(graph.scope, "ready", source), current);
  return current;
}
export async function historyFlags(
  graph: Graph,
  source: string,
): Promise<
  {
    has_streaming: boolean;
    has_legacy_context: boolean;
    terminal_activity: boolean;
  }
> {
  const current = await initialize(graph, source);
  const count = async (facet: Facet) => {
    const value = await graph.tx.get<number>(
      k(graph.scope, "count", source, current.generation, facet),
    );
    if (!Number.isSafeInteger(value) || Number(value) < 0) {
      throw new Error("missing history flag count");
    }
    return Number(value) > 0;
  };
  return {
    has_streaming: await count("streaming"),
    has_legacy_context: await count("legacy"),
    terminal_activity: await count("terminal"),
  };
}
