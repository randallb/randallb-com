import catalog from "./migration/url-catalog.json" with { type: "json" };
const paths = new Set(catalog.map((entry) => entry.path));
const page =
  '<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>randallb.com coming soon</title><body>randallb.com coming soon</body></html>';
export default {
  fetch(request: Request): Response {
    const path = new URL(request.url).pathname;
    if (!["GET", "HEAD"].includes(request.method)) {
      return new Response(null, {
        status: 405,
        headers: { Allow: "GET, HEAD" },
      });
    }
    return new Response(request.method === "HEAD" ? null : page, {
      status: paths.has(path) ? 200 : 404,
      headers: {
        "Content-Type": "text/html; charset=utf-8",
        "Cache-Control": "no-store",
        "X-Robots-Tag": "noindex",
      },
    });
  },
};
