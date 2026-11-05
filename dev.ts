// Run the Drive build first; its generated entry supplies the content.
const { default: worker } = await import("./.drive-build/entry.ts");
Deno.serve(
  { hostname: "127.0.0.1", port: 8000 },
  (request) => worker.fetch(request),
);
