import app from "./worker.ts";
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
Deno.test("preview identifies its deployed revision and discourages indexing", async () => {
  const revision = "a".repeat(40);
  const page = app.fetch(new Request("https://preview.example/"), {
    SOURCE_REVISION: revision,
  });
  assert(page.status === 200, "page should load");
  assert(
    page.headers.get("x-robots-tag") === "noindex, nofollow",
    "preview must not be indexed",
  );
  assert((await page.text()).includes(revision), "page must identify revision");
  const health = await app.fetch(
    new Request("https://preview.example/healthz"),
    { SOURCE_REVISION: revision },
  ).json();
  assert(
    health.revision === revision && health.environment === "preview",
    "health must match deployed revision",
  );
});
Deno.test("routing supports HEAD and rejects unknown paths and writes", async () => {
  const head = app.fetch(
    new Request("https://preview.example/", { method: "HEAD" }),
  );
  assert(
    head.status === 200 && (await head.text()) === "",
    "HEAD must have no body",
  );
  assert(
    app.fetch(new Request("https://preview.example/missing")).status === 404,
    "unknown paths must return 404",
  );
  const write = app.fetch(
    new Request("https://preview.example/", { method: "POST" }),
  );
  assert(
    write.status === 405 && write.headers.get("allow") === "GET, HEAD",
    "writes must be rejected",
  );
});
