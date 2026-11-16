import type { Transaction } from "./engine.ts";
/** Immutable logical binding shared by declared domains in one workspace owner.
 * Keys are private backend details, not a second authority mechanism. */
export async function bindIdentity(
  tx: Transaction,
  reference: string,
  kind: string,
  owner: string,
  schema: string,
): Promise<void> {
  const key = `identity:${reference}`;
  const old = await tx.get<{ kind: string; owner: string; schema: string }>(
    key,
  );
  if (
    old && (old.kind !== kind || old.owner !== owner || old.schema !== schema)
  ) throw new Error("ref_retired_or_unavailable: canonical identity collision");
  if (!old) await tx.put(key, { kind, owner, schema });
  await retainReference(tx, reference);
}

export async function retainReference(
  tx: Transaction,
  reference: string,
): Promise<number> {
  const key = `ordinal:${reference}`;
  const old = await tx.get<number>(key);
  if (old !== undefined) return old;
  const next = (await tx.get<number>("identity-ordinal-counter") ?? 0) + 1;
  if (!Number.isSafeInteger(next)) throw new Error("retained order exhausted");
  await tx.put("identity-ordinal-counter", next);
  await tx.put(key, next);
  return next;
}
