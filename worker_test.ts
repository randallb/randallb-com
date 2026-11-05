import worker from "./worker.ts";
import catalog from "./migration/url-catalog.json" with { type: "json" };
Deno.test("cataloged URLs serve a placeholder, preserving query and HEAD behavior", async () => {
  for (const row of catalog) {
    const response = worker.fetch(
      new Request("https://randallb.com" + row.path + "?source=test"),
    );
    if (
      response.status !== 200 ||
      !(await response.text()).includes("<body>randallb.com coming soon</body>")
    ) throw new Error(row.path);
  }
  if (
    await worker.fetch(new Request("https://randallb.com/", { method: "HEAD" }))
      .text() !== ""
  ) throw new Error("HEAD body");
});
Deno.test("unknown pages remain 404 and forms cannot submit", () => {
  if (
    worker.fetch(new Request("https://randallb.com/missing")).status !== 404
  ) throw new Error("404");
  if (
    worker.fetch(
      new Request("https://randallb.com/subscribe", {
        method: "POST",
        body: "test",
      }),
    ).status !== 405
  ) throw new Error("POST");
});
