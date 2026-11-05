declare module "cloudflare:workers" {
  export class DurableObject<E = unknown> {
    protected env: E;
    protected ctx: { storage: import("./kv.ts").KvStorage };
    constructor(ctx: { storage: import("./kv.ts").KvStorage }, env: E);
  }
}
