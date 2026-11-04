export interface Env {
  SOURCE_REVISION?: string;
}

const headers = {
  "cache-control": "no-store",
  "x-robots-tag": "noindex, nofollow",
  "x-content-type-options": "nosniff",
  "referrer-policy": "no-referrer",
  "content-security-policy":
    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
};

export default {
  fetch(request: Request, env: Env = {}): Response {
    const url = new URL(request.url);
    const revision = /^[a-f0-9]{40}$/.test(env.SOURCE_REVISION ?? "")
      ? env.SOURCE_REVISION!
      : "local";
    const respond = (
      body: string,
      status = 200,
      type = "text/plain; charset=utf-8",
      extra = {},
    ) =>
      new Response(request.method === "HEAD" ? null : body, {
        status,
        headers: { ...headers, "content-type": type, ...extra },
      });
    if (!["GET", "HEAD"].includes(request.method)) {
      return respond("Method not allowed", 405, undefined, {
        allow: "GET, HEAD",
      });
    }
    if (url.pathname === "/healthz") {
      return respond(
        JSON.stringify({
          status: "ok",
          site: "randallb.com",
          environment: "preview",
          revision,
        }),
        200,
        "application/json; charset=utf-8",
      );
    }
    if (url.pathname === "/robots.txt") {
      return respond("User-agent: *\nDisallow: /\n");
    }
    if (url.pathname !== "/") return respond("Not found", 404);
    return respond(
      `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="robots" content="noindex,nofollow"><title>randallb.com — Preview</title>
<style>
:root{color-scheme:light dark;font-family:system-ui,sans-serif;background:#f5f3ed;color:#242a25}*{box-sizing:border-box}body{margin:0;min-height:100svh;display:grid;place-items:center;padding:clamp(24px,6vw,80px)}main{width:min(100%,760px)}.label{font-size:.75rem;letter-spacing:.14em;text-transform:uppercase;color:#52695b}h1{font-family:Georgia,serif;font-size:clamp(3rem,9vw,6rem);font-weight:400;letter-spacing:-.055em;line-height:1.05;margin:1.5rem 0}p{max-width:44ch;font-size:1.15rem;line-height:1.65}.status{margin-top:3rem;padding-top:1.5rem;border-top:1px solid #c7cdc5;color:#52695b;font-size:.9rem}code{overflow-wrap:anywhere;font-size:.8rem}@media(prefers-color-scheme:dark){:root{background:#18211b;color:#f5f3ed}.label,.status{color:#b7ccba}.status{border-color:#425447}}
</style></head><body><main><span class="label">Development preview</span><h1>randallb.com</h1><p>A new home for useful ideas and tools.</p><p>This preview verifies the standalone application and hosting. The public site remains available at its current address.</p><footer class="status">Preview is running.<br>Source revision: <code>${revision}</code></footer></main></body></html>`,
      200,
      "text/html; charset=utf-8",
    );
  },
};
