import type { Context, Mutation, Outcome, StoredRecord } from "./client.ts";
import { bindIdentity } from "./identity.ts";
export interface Transaction {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<void>;
}
export interface Storage extends Transaction {
  transaction<T>(work: (transaction: Transaction) => Promise<T>): Promise<T>;
}
export type Authorizer = (
  context: Context,
  reference: string,
  write: boolean,
) => Promise<void>;
export type Validator = (
  schema: string,
  previous: Record<string, unknown> | null,
  next: Record<string, unknown>,
) => Promise<void>;
interface Node {
  workspace: string;
  counter: number;
  record: StoredRecord;
}
interface Receipt {
  digest?: string;
  outcome?: Outcome;
}
export function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${
      Object.entries(value).filter(([, v]) => v !== undefined).sort((
        [a],
        [b],
      ) => a < b ? -1 : a > b ? 1 : 0).map(([k, v]) =>
        `${JSON.stringify(k)}:${canonical(v)}`
      ).join(",")
    }}`;
  }
  return JSON.stringify(value);
}
export async function digest(value: unknown): Promise<string> {
  const bytes = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(canonical(value)),
  );
  return [...new Uint8Array(bytes)].map((n) => n.toString(16).padStart(2, "0"))
    .join("");
}
function validRef(reference: string): void {
  if (
    typeof reference !== "string" || !reference.startsWith("cwp://") ||
    reference.length > 4096 || reference.includes("#")
  ) throw new Error("invalid canonical storage reference");
}
export function validContext(context: Context, reference: string): void {
  validRef(context.workspace_ref);
  validRef(context.caller_ref);
  validRef(reference);
}
function object(value: unknown): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("invalid object value");
  }
}
function merge(target: unknown, patch: unknown): unknown {
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) {
    return patch;
  }
  const result: Record<string, unknown> =
    target !== null && typeof target === "object" && !Array.isArray(target)
      ? { ...target }
      : {};
  for (const [key, value] of Object.entries(patch)) {
    if (key === "__proto__") {
      Object.defineProperty(result, key, {
        value: value === null ? undefined : merge(result[key], value),
        writable: true,
        enumerable: value !== null,
        configurable: true,
      });
      continue;
    }
    if (value === null) delete result[key];
    else result[key] = merge(result[key], value);
  }
  return result;
}
/** Shared host-neutral declared operation engine. The Durable Object adapter
 * owns physical keys/chunks/transactions. Domain callers supply only refs. */
export class Documents {
  constructor(
    private readonly storage: Storage,
    private readonly authorize: Authorizer,
    private readonly validate: Validator,
  ) {}
  async get(context: Context, reference: string): Promise<StoredRecord | null> {
    validContext(context, reference);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(context, reference, false);
      const node = await tx.get<Node>(`node:${reference}`);
      return node?.workspace === context.workspace_ref ? node.record : null;
    });
  }
  async reconcile(input: Mutation): Promise<Outcome | null> {
    const request = {
      ...input,
      expected_revision: input.expected_revision ?? null,
    };
    validContext(request.context, request.reference);
    if (!request.request_id || request.request_id.length > 128) {
      throw new Error("invalid mutation request ID");
    }
    return await this.storage.transaction(async (tx) => {
      await this.authorize(request.context, request.reference, true);
      const key = `receipt:${await digest([
        request.context.workspace_ref,
        request.context.caller_ref,
        request.request_id,
      ])}`;
      const receipt = await tx.get<Receipt>(key);
      if (!receipt) return null;
      if (!receipt.outcome) return { outcome: "committed_result_expired" };
      if (receipt.digest !== await digest(request)) {
        throw new Error("mutation request conflict");
      }
      return receipt.outcome;
    });
  }
  async mutate(input: Mutation): Promise<Outcome> {
    const request = {
      ...input,
      expected_revision: input.expected_revision ?? null,
    };
    validContext(request.context, request.reference);
    if (!request.request_id || request.request_id.length > 128) {
      throw new Error("invalid mutation request ID");
    }
    if (new TextEncoder().encode(canonical(request)).length > 524288) {
      throw new Error("document mutation payload exceeds 512 KiB");
    }
    return await this.storage.transaction(async (tx) => {
      await this.authorize(request.context, request.reference, true);
      const receiptKey = `receipt:${await digest([
        request.context.workspace_ref,
        request.context.caller_ref,
        request.request_id,
      ])}`;
      const argumentsDigest = await digest(request);
      const receipt = await tx.get<Receipt>(receiptKey);
      if (receipt) {
        if (!receipt.outcome) return { outcome: "committed_result_expired" };
        if (receipt.digest !== argumentsDigest) {
          throw new Error("mutation request conflict");
        }
        return receipt.outcome;
      }
      const nodeKey = `node:${request.reference}`;
      const old = await tx.get<Node>(nodeKey);
      const previous = old?.workspace === request.context.workspace_ref
        ? old
        : undefined;
      const operation = request.operation;
      let outcome: Outcome;
      if (operation.operation === "delete") {
        if (!previous) throw new Error("not_found");
        if (request.expected_revision !== previous.record.revision) {
          throw new Error("revision_conflict");
        }
        // These records currently have no public relationship operations. Any
        // future edge domain must register authoritative incident-edge checks.
        await tx.delete(nodeKey);
        const dependencies =
          await tx.get<string[]>(`dependencies:${request.reference}`) ?? [];
        for (const key of dependencies) await tx.put<Receipt>(key, {});
        await tx.delete(`dependencies:${request.reference}`);
        outcome = {
          outcome: "committed",
          record: null,
          deleted_revision: previous.record.revision,
        };
      } else {
        let schema: string;
        let value: Record<string, unknown>;
        if (operation.operation === "create") {
          validRef(operation.schema_ref);
          object(operation.value);
          if (request.expected_revision !== null) {
            throw new Error("create cannot name an expected revision");
          }
          if (previous) throw new Error("already_exists");
          if (old || await tx.get<boolean>(`retired:${request.reference}`)) {
            throw new Error("ref_retired_or_unavailable");
          }
          schema = operation.schema_ref;
          await bindIdentity(
            tx,
            request.reference,
            "cloud_document_v1",
            request.context.workspace_ref,
            schema,
          );
          value = operation.value;
        } else {
          if (!previous) throw new Error("not_found");
          if (request.expected_revision !== previous.record.revision) {
            throw new Error("revision_conflict");
          }
          schema = previous.record.schema_ref;
          if (operation.operation === "replace") {
            object(operation.value);
            value = operation.value;
          } else if (operation.operation === "patch") {
            object(operation.patch);
            value = merge(previous.record.value, operation.patch) as Record<
              string,
              unknown
            >;
          } else throw new Error("unsupported document operation");
        }
        await this.validate(schema, previous?.record.value ?? null, value);
        const unchanged = previous &&
          canonical(previous.record.value) === canonical(value);
        const counter = previous ? previous.counter + (unchanged ? 0 : 1) : 0;
        if (!Number.isSafeInteger(counter)) {
          throw new Error("revision exhausted");
        }
        const record: StoredRecord = {
          reference: request.reference,
          schema_ref: schema,
          value,
          revision: await digest([request.reference, counter]),
        };
        await tx.put<Node>(nodeKey, {
          workspace: request.context.workspace_ref,
          counter,
          record,
        });
        await tx.put(`retired:${request.reference}`, true);
        outcome = { outcome: "committed", record };
      }
      const dependencies =
        await tx.get<string[]>(`dependencies:${request.reference}`) ?? [];
      if (dependencies.length >= 1023) {
        throw new Error("receipt dependency work limit exceeded");
      }
      await tx.put(`dependencies:${request.reference}`, [
        ...dependencies,
        receiptKey,
      ]);
      if (new TextEncoder().encode(canonical(outcome)).length > 524288) {
        throw new Error("mutation receipt limit exceeded");
      }
      await tx.put<Receipt>(receiptKey, { digest: argumentsDigest, outcome });
      return outcome;
    });
  }
}
