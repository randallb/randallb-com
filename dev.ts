import app from "./worker.ts";
Deno.serve(
  { hostname: "127.0.0.1", port: 8000 },
  (request) => app.fetch(request),
);
