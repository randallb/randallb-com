/** cwpdb declared storage port. A trusted host binds context and authenticates
 * transport; these context fields are never authentication on their own. */
export interface Context {
  workspace_ref: string;
  caller_ref: string;
}
export interface StoredRecord {
  reference: string;
  schema_ref: string;
  value: Record<string, unknown>;
  revision: string;
}
export type DocumentOperation =
  | { operation: "create"; schema_ref: string; value: Record<string, unknown> }
  | { operation: "replace"; value: Record<string, unknown> }
  | { operation: "patch"; patch: Record<string, unknown> }
  | { operation: "delete" };
export interface Mutation {
  context: Context;
  reference: string;
  request_id: string;
  expected_revision?: string | null;
  operation: DocumentOperation;
}
export type Outcome = {
  outcome: "committed";
  record: StoredRecord | null;
  deleted_revision?: string;
} | { outcome: "committed_result_expired" };
export interface DocumentStore {
  get(context: Context, reference: string): Promise<StoredRecord | null>;
  mutate(request: Mutation): Promise<Outcome>;
}
/** Transport is supplied by a trusted gateway/service binding, not a user URL. */
export class Client implements DocumentStore {
  constructor(
    private readonly transport: (body: unknown) => Promise<unknown>,
  ) {}
  async get(context: Context, reference: string): Promise<StoredRecord | null> {
    return await this.transport({
      operation: "get_document",
      context,
      reference,
    }) as StoredRecord | null;
  }
  async mutate(request: Mutation): Promise<Outcome> {
    return await this.transport({
      operation: "mutate_document",
      request,
    }) as Outcome;
  }
}
