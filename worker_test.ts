import { createWorker } from "./worker.ts";

const file = { body: btoa("fixture page"), content_type: "text/html" };
const worker = createWorker({
  revision: "fixture",
  noindex: true,
  routes: { "/": file, "/example": file },
  not_found: file,
});

Deno.test("Drive route content, query and HEAD behavior", async () => {
  const response = worker.fetch(
    new Request("https://example.test/example?q=1"),
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
  const head = worker.fetch(
    new Request("https://example.test/", {
      method: "HEAD",
    }),
  );
  if (head.status !== 200 || await head.text() !== "") {
    throw new Error("HEAD body");
  }
});

Deno.test("unlisted files remain private and forms cannot submit", () => {
  for (const path of ["/missing", "/reference/page.html", "/toString"]) {
    if (
      worker.fetch(new Request("https://example.test" + path)).status !== 404
    ) {
      throw new Error("404");
    }
  }
  if (
    worker.fetch(new Request("https://example.test/", { method: "POST" }))
      .status !== 405
  ) throw new Error("POST");
});

Deno.test("binary assets retain their bytes", async () => {
  const binary = { body: "AP+A", content_type: "image/png" };
  const assets = createWorker({
    revision: "binary",
    noindex: false,
    routes: { "/asset.png": binary },
    not_found: file,
  });
  const response = assets.fetch(new Request("https://example.test/asset.png"));
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.join(",") !== "0,255,128") throw new Error("binary bytes");
  if (response.headers.has("X-Robots-Tag")) throw new Error("index setting");
});
