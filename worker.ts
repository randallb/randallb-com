export interface SiteFile {
  body: string;
  content_type: string;
}
export interface Site {
  revision: string;
  noindex: boolean;
  routes: Record<string, SiteFile>;
  not_found: SiteFile;
}

// Content is supplied by the Drive build, never by repository page files.
export function createWorker(site: Site) {
  return {
    fetch(request: Request): Response {
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
      if (site.noindex) headers["X-Robots-Tag"] = "noindex";
      const body = request.method === "HEAD"
        ? null
        : Uint8Array.from(atob(file.body), (char) => char.charCodeAt(0));
      return new Response(body, { status: found ? 200 : 404, headers });
    },
  };
}
