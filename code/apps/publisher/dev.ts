// Local static asset binding for the generated Drive build.
const { default: worker } = await import("./.drive-build/entry.ts");
const env = {
  ASSETS: {
    async fetch(request: Request) {
      const name = new URL(request.url).pathname;
      if (!/^\/[a-f0-9]{64}\.bin$/.test(name)) {
        return new Response(null, {
          status: 404,
        });
      }
      try {
        return new Response(
          await Deno.readFile(
            new URL("./.drive-build/assets" + name, import.meta.url),
          ),
        );
      } catch {
        return new Response(null, { status: 404 });
      }
    },
  },
};
Deno.serve(
  { hostname: "127.0.0.1", port: Number(Deno.args[0] ?? 8000) },
  (request) => worker.fetch(request, env),
);
