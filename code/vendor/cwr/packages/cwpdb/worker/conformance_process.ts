type Transport = (body: unknown) => Promise<unknown>;
export async function processTransport(
  args: string[],
  binary: string,
): Promise<{ call: Transport; close: () => Promise<void> }> {
  const child = new Deno.Command(binary, {
    args,
    stdin: "piped",
    stdout: "piped",
    stderr: "inherit",
  }).spawn();
  const writer = child.stdin.getWriter();
  const reader = child.stdout.pipeThrough(new TextDecoderStream()).getReader();
  let buffered = "";
  return {
    call: async (body) => {
      await writer.write(new TextEncoder().encode(JSON.stringify(body) + "\n"));
      while (!buffered.includes("\n")) {
        const read = await reader.read();
        if (read.done) throw new Error("fixture process stopped");
        buffered += read.value;
      }
      const end = buffered.indexOf("\n");
      const response = JSON.parse(buffered.slice(0, end));
      buffered = buffered.slice(end + 1);
      if (!response.ok) throw new Error(response.error);
      return response.result;
    },
    close: async () => {
      await writer.close();
      await reader.cancel();
      const status = await child.status;
      if (!status.success) throw new Error("fixture failed");
    },
  };
}
