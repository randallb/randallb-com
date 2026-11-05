import worker from "./worker.ts";
Deno.serve(
  { hostname: "127.0.0.1", port: 8000 },
  (request) => worker.fetch(request),
);
