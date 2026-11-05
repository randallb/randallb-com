import type { Context } from "./client.ts";
import {
  type Authorizer,
  canonical,
  digest,
  type Storage,
  type Transaction,
  validContext,
} from "./engine.ts";
export interface Assignment {
  executor_ref: string;
  assignment_ref: string;
  generation: number;
  route_fingerprint: string;
  expires_at_unix_ms: number;
}
export type EventKind = "progress" | "completed" | "interrupted" | "cancelled";
export interface Event {
  sequence: number;
  kind: EventKind;
  payload: unknown;
}
export type Operation =
  | {
    operation: "admit";
    conversation_ref: string;
    parent_invocation_ref: string | null;
    admitted: unknown;
  }
  | { operation: "assign"; assignment: Assignment }
  | {
    operation: "append";
    assignment_ref: string;
    executor_ref: string;
    generation: number;
    event: Event;
  }
  | { operation: "stop"; kind: EventKind; reason: unknown };
export interface Mutation {
  context: Context;
  invocation_ref: string;
  request_id: string;
  expected_revision: string | null;
  operation: Operation;
}
export interface InvocationRecord {
  workspace_ref: string;
  invocation_ref: string;
  conversation_ref: string;
  parent_invocation_ref: string | null;
  admitted: unknown;
  assignment: Assignment | null;
  revision: string;
  next_sequence: number;
  terminal: EventKind | null;
}
export interface Receipt {
  invocation_ref: string;
  revision: string;
  next_sequence: number;
  terminal: EventKind | null;
}
export type Outcome = { outcome: "committed"; receipt: Receipt } | {
  outcome: "committed_result_expired";
};
interface Node {
  counter: number;
  record: InvocationRecord;
}
interface SavedReceipt {
  digest: string;
  outcome: Outcome;
  request: Mutation;
}
export class Invocations {
  constructor(
    private readonly storage: Storage,
    private readonly authorize: Authorizer,
  ) {}
  private async key(context: Context, invocation: string): Promise<string> {
    return `invocation:${await digest([context.workspace_ref, invocation])}`;
  }
  async get(
    context: Context,
    invocation_ref: string,
  ): Promise<InvocationRecord | null> {
    validContext(context, invocation_ref);
    return await this.storage.transaction(async (tx) => {
      await this.authorize(context, invocation_ref, false);
      return (await tx.get<Node>(await this.key(context, invocation_ref)))
        ?.record ?? null;
    });
  }
  async receipt(
    context: Context,
    invocation_ref: string,
    request_id: string,
  ): Promise<unknown> {
    validContext(context, invocation_ref);
    if (!request_id || request_id.length > 128) {
      throw new Error("invalid mutation request ID");
    }
    return await this.storage.transaction(async (tx) => {
      await this.authorize(context, invocation_ref, false);
      const key = `receipt:${await digest([
        context.workspace_ref,
        context.caller_ref,
        request_id,
      ])}`;
      const saved = await tx.get<SavedReceipt>(key);
      if (!saved) {
        await this.authorize(context, invocation_ref, false);
        return { outcome: "not_committed" };
      }
      if (!saved.outcome) {
        await this.authorize(context, invocation_ref, false);
        return { outcome: "committed_result_expired" };
      }
      if (
        saved.request?.invocation_ref !== invocation_ref ||
        saved.outcome.outcome !== "committed"
      ) throw new Error("mutation request conflict");
      await this.authorize(context, invocation_ref, false);
      return {
        outcome: "retained",
        request: saved.request,
        receipt: saved.outcome.receipt,
      };
    });
  }
  async mutate(request: Mutation, now_unix_ms: number): Promise<Outcome> {
    validContext(request.context, request.invocation_ref);
    if (!request.request_id || request.request_id.length > 128) {
      throw new Error("invalid mutation request ID");
    }
    if (!Number.isSafeInteger(now_unix_ms) || now_unix_ms < 0) {
      throw new Error("invalid coordinator clock");
    }
    if (new TextEncoder().encode(canonical(request)).length > 524288) {
      throw new Error("invocation mutation payload exceeds 512 KiB");
    }
    return await this.storage.transaction(async (tx) => {
      await this.authorize(request.context, request.invocation_ref, true);
      const receiptKey = `receipt:${await digest([
        request.context.workspace_ref,
        request.context.caller_ref,
        request.request_id,
      ])}`;
      const argumentsDigest = await digest(request);
      const saved = await tx.get<SavedReceipt>(receiptKey);
      if (saved) {
        if (!saved.outcome) {
          await this.authorize(request.context, request.invocation_ref, true);
          return { outcome: "committed_result_expired" };
        }
        if (saved.digest !== argumentsDigest) {
          throw new Error("mutation request conflict");
        }
        await this.authorize(request.context, request.invocation_ref, true);
        return saved.outcome;
      }
      const key = await this.key(request.context, request.invocation_ref);
      const previous = await tx.get<Node>(key);
      const operation = request.operation;
      let record: InvocationRecord;
      let event: Event | undefined;
      if (operation.operation === "admit") {
        validContext(request.context, operation.conversation_ref);
        if (operation.parent_invocation_ref) {
          validContext(request.context, operation.parent_invocation_ref);
        }
        if (request.expected_revision !== null) {
          throw new Error("admission cannot name an expected revision");
        }
        if (previous) throw new Error("invocation already admitted");
        record = {
          workspace_ref: request.context.workspace_ref,
          invocation_ref: request.invocation_ref,
          conversation_ref: operation.conversation_ref,
          parent_invocation_ref: operation.parent_invocation_ref,
          admitted: operation.admitted,
          assignment: null,
          revision: "",
          next_sequence: 0,
          terminal: null,
        };
      } else {
        if (!previous) throw new Error("invocation not found");
        record = structuredClone(previous.record);
        if (request.expected_revision !== record.revision) {
          throw new Error("invocation revision conflict");
        }
        if (record.terminal !== null) throw new Error("invocation is terminal");
        if (operation.operation === "assign") {
          validContext(request.context, operation.assignment.executor_ref);
          validContext(request.context, operation.assignment.assignment_ref);
          const a = operation.assignment;
          if (record.assignment) {
            throw new Error("invocation assignment is immutable");
          }
          if (
            !Number.isSafeInteger(a.generation) || a.generation < 1 ||
            !Number.isSafeInteger(a.expires_at_unix_ms) ||
            a.expires_at_unix_ms <= now_unix_ms || !a.route_fingerprint
          ) throw new Error("invalid invocation assignment");
          record.assignment = a;
        } else if (operation.operation === "append") {
          const a = record.assignment;
          if (
            !a || a.assignment_ref !== operation.assignment_ref ||
            a.executor_ref !== operation.executor_ref ||
            a.generation !== operation.generation ||
            a.expires_at_unix_ms <= now_unix_ms
          ) throw new Error("stale or foreign invocation assignment");
          if (operation.event.sequence !== record.next_sequence) {
            throw new Error("invocation event sequence conflict");
          }
          event = operation.event;
        } else if (operation.operation === "stop") {
          if (
            operation.kind !== "interrupted" && operation.kind !== "cancelled"
          ) {
            throw new Error(
              "coordinator stop requires interruption or cancellation",
            );
          }
          event = {
            sequence: record.next_sequence,
            kind: operation.kind,
            payload: operation.reason,
          };
        } else throw new Error("unsupported invocation operation");
        if (event) {
          if (
            !["progress", "completed", "interrupted", "cancelled"].includes(
              event.kind,
            )
          ) throw new Error("invalid event kind");
          record.next_sequence++;
          if (!Number.isSafeInteger(record.next_sequence)) {
            throw new Error("invocation sequence exhausted");
          }
          if (event.kind !== "progress") record.terminal = event.kind;
        }
      }
      const counter = previous ? previous.counter + 1 : 0;
      if (!Number.isSafeInteger(counter)) throw new Error("revision exhausted");
      // Physical revision tokens are opaque across backend instances. The native
      // backend's graph key differs, while every compare/replay rule is equal.
      record.revision = await digest([key, counter]);
      await tx.put<Node>(key, { counter, record });
      if (event) await tx.put(`${key}:event:${event.sequence}`, event);
      const receipt: Receipt = {
        invocation_ref: record.invocation_ref,
        revision: record.revision,
        next_sequence: record.next_sequence,
        terminal: record.terminal,
      };
      const outcome: Outcome = { outcome: "committed", receipt };
      if (
        new TextEncoder().encode(canonical({ request, receipt })).length >
          524288
      ) throw new Error("mutation receipt limit exceeded");
      await tx.put<SavedReceipt>(receiptKey, {
        digest: argumentsDigest,
        outcome,
        request,
      });
      await this.authorize(request.context, request.invocation_ref, true);
      return outcome;
    });
  }
  async events(
    context: Context,
    invocation_ref: string,
    after: number | null,
    limit: number,
  ): Promise<
    { items: Event[]; continuation: number | null; complete: boolean }
  > {
    validContext(context, invocation_ref);
    if (
      !Number.isInteger(limit) || limit < 1 || limit > 1000 ||
      after !== null && (!Number.isSafeInteger(after) || after < 0)
    ) {
      throw new Error(
        "invocation page requires limit 1..1000 and a valid sequence",
      );
    }
    return await this.storage.transaction(async (tx: Transaction) => {
      await this.authorize(context, invocation_ref, false);
      const key = await this.key(context, invocation_ref);
      const node = await tx.get<Node>(key);
      if (!node) throw new Error("invocation not found");
      const items: Event[] = [];
      for (
        let i = (after ?? -1) + 1;
        i < node.record.next_sequence && items.length < limit;
        i++
      ) {
        const event = await tx.get<Event>(`${key}:event:${i}`);
        if (!event) throw new Error("incomplete invocation event sequence");
        items.push(event);
      }
      const complete = !items.length ||
        items[items.length - 1].sequence + 1 >= node.record.next_sequence;
      return {
        items,
        continuation: complete ? null : items[items.length - 1].sequence,
        complete,
      };
    });
  }
}
