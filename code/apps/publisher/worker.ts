export interface SiteFile {
  asset: string;
  content_type: string;
  noindex?: boolean;
}
export interface Site {
  revision: string;
  noindex: boolean;
  routes: Record<string, SiteFile>;
  not_found: SiteFile;
}
export interface AssetEnvironment {
  PREVIEW_ONLY?: boolean;
  ASSETS: { fetch(request: Request): Promise<Response> };
}

// Only explicit Drive routes can reach the uploaded, content-addressed assets.
export function createWorker(site: Site) {
  return {
    async fetch(request: Request, env: AssetEnvironment): Promise<Response> {
      if (!["GET", "HEAD"].includes(request.method)) {
        return new Response(null, {
          status: 405,
          headers: { Allow: "GET, HEAD" },
        });
      }
      const path = new URL(request.url).pathname;
      const found = Object.hasOwn(site.routes, path);
      const file = found ? site.routes[path] : site.not_found;
      const headers: Record<string, string> = {
        "Content-Type": file.content_type,
        "Cache-Control": "no-store",
        "X-Content-Revision": site.revision,
      };
      if (env.PREVIEW_ONLY || (file.noindex ?? site.noindex)) {
        headers["X-Robots-Tag"] = "noindex";
      }
      if (request.method === "HEAD") {
        return new Response(null, { status: found ? 200 : 404, headers });
      }
      const assetUrl = new URL(file.asset, request.url);
      assetUrl.search = "";
      const asset = await env.ASSETS.fetch(new Request(assetUrl));
      if (asset.status !== 200) {
        return new Response("Asset unavailable", { status: 502 });
      }
      return new Response(asset.body, { status: found ? 200 : 404, headers });
    },
  };
}
