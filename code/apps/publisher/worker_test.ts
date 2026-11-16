import { createWorker } from "./worker.ts";

const file = { asset: "/fixture.bin", content_type: "text/html" };
const env = {
  ASSETS: {
    fetch: (request: Request) =>
      Promise.resolve(
        new Response(
          new URL(request.url).pathname === "/binary.bin"
            ? new Uint8Array([0, 255, 128])
            : "fixture page",
        ),
      ),
  },
};
const worker = createWorker({
  revision: "fixture",
  noindex: true,
  routes: { "/": file, "/example": file },
  not_found: file,
});

Deno.test("Drive route content, query and HEAD behavior", async () => {
  const response = await worker.fetch(
    new Request("https://example.test/example?q=1"),
    env,
  );
  if (response.status !== 200 || await response.text() !== "fixture page") {
    throw new Error("route content");
  }
  if (response.headers.get("X-Content-Revision") !== "fixture") {
    throw new Error("revision");
  }
  if (response.headers.get("X-Robots-Tag") !== "noindex") {
    throw new Error("noindex");
  }
  const head = await worker.fetch(
    new Request("https://example.test/", {
      method: "HEAD",
    }),
    env,
  );
  if (head.status !== 200 || await head.text() !== "") {
    throw new Error("HEAD body");
  }
});

Deno.test("unlisted files remain private and forms cannot submit", async () => {
  for (const path of ["/missing", "/reference/page.html", "/toString"]) {
    if (
      (await worker.fetch(new Request("https://example.test" + path), env))
        .status !== 404
    ) {
      throw new Error("404");
    }
  }
  if (
    (await worker.fetch(
      new Request("https://example.test/", { method: "POST" }),
      env,
    ))
      .status !== 405
  ) throw new Error("POST");
});

Deno.test("binary assets retain their bytes", async () => {
  const binary = { asset: "/binary.bin", content_type: "image/png" };
  const assets = createWorker({
    revision: "binary",
    noindex: false,
    routes: { "/asset.png": binary },
    not_found: file,
  });
  const response = await assets.fetch(
    new Request("https://example.test/asset.png"),
    env,
  );
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.join(",") !== "0,255,128") throw new Error("binary bytes");
  if (response.headers.has("X-Robots-Tag")) throw new Error("index setting");
});

Deno.test("preview is noindex and unavailable assets fail clearly", async () => {
  const preview = await worker.fetch(new Request("https://example.test/"), {
    ...env,
    PREVIEW_ONLY: true,
  });
  if (preview.headers.get("X-Robots-Tag") !== "noindex") {
    throw new Error("preview indexing");
  }
  await preview.body?.cancel();
  const failed = await worker.fetch(new Request("https://example.test/"), {
    ASSETS: {
      fetch: () => Promise.resolve(new Response(null, { status: 404 })),
    },
  });
  if (failed.status !== 502) throw new Error("missing asset");
});
