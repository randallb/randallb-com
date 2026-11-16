/** Local conformance entrypoint only. This is not the deployable worker. */
export { WorkspaceStorage } from "./worker.ts";
interface Env {
  WORKSPACE_STORAGE: {
    idFromName(name: string): unknown;
    get(
      id: unknown,
    ): { dispatch(token: string, body: unknown): Promise<unknown> };
  };
  CWPDB_COORDINATOR_TOKEN: string;
}
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (
      url.protocol !== "http:" ||
      !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)
    ) return new Response("Denied", { status: 403 });
    if (
      request.method !== "POST" ||
      request.headers.get("authorization") !==
        `Bearer ${env.CWPDB_COORDINATOR_TOKEN}`
    ) return new Response("Denied", { status: 403 });
    try {
      const body = await request.json() as {
        context?: { workspace_ref: string };
        request?: {
          context?: { workspace_ref: string };
          scope?: { context: { workspace_ref: string } };
        };
        scope?: { context: { workspace_ref: string } };
        invocation_request?: {
          context?: { workspace_ref: string };
          scope?: { context: { workspace_ref: string } };
        };
      };
      const context = body.request?.scope?.context ?? body.scope?.context ??
        body.request?.context ??
        body.invocation_request?.context ?? body.context;
      if (!context) throw new Error("missing context");
      const object = env.WORKSPACE_STORAGE.get(
        env.WORKSPACE_STORAGE.idFromName(context.workspace_ref),
      );
      return Response.json({
        ok: true,
        result: await object.dispatch(env.CWPDB_COORDINATOR_TOKEN, body),
      });
    } catch (error) {
      return Response.json({
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },
};
