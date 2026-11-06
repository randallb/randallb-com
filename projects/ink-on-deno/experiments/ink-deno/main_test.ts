import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const artifacts = join(root, "artifacts");
const quote = (value: string) => `'${value.replaceAll("'", "'\"'\"'")}'`;
const delay = () => new Promise((resolve) => setTimeout(resolve, 50));

async function run(command: string, args: string[], cwd = root) {
  const output = await new Deno.Command(command, {
    args,
    cwd,
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert.equal(output.code, 0, new TextDecoder().decode(output.stderr));
  return new TextDecoder().decode(output.stdout);
}

async function until<T>(read: () => Promise<T>, ready: (value: T) => boolean) {
  const deadline = Date.now() + 10_000;
  let value: T;
  do {
    value = await read();
    if (ready(value)) return value;
    await delay();
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for complete state: ${String(value)}`);
}

async function rejectsNonTty(command: string, args: string[]) {
  const output = await new Deno.Command(command, {
    args,
    cwd: root,
    stdin: "null",
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert.equal(output.code, 1);
  assert.equal(output.stdout.length, 0);
  assert.equal(
    new TextDecoder().decode(output.stderr).trim(),
    "This synthetic demo needs an interactive terminal.",
  );
}

async function terminalCase(
  name: string,
  argv: string[],
  termination = "ctrl-c",
) {
  // Unix sockets have a short path limit; the managed checkout path is too long.
  const temporary = await Deno.makeTempDir({ dir: "/tmp", prefix: "ink-pty-" });
  const socket = join(temporary, "tmux.sock");
  const tmux = (...args: string[]) => run("tmux", ["-S", socket, ...args]);
  const screen = () => tmux("capture-pane", "-p", "-t", "demo");
  const contains = (text: string, ...items: string[]) =>
    items.every((item) => text.includes(item));
  const waitScreen = (ready: (text: string) => boolean) => until(screen, ready);
  const cache = await Deno.makeTempDir({
    dir: temporary,
    prefix: "empty-cache-",
  });
  const config = join(temporary, "tmux.conf");
  const before = join(temporary, "before");
  const after = join(temporary, "after");
  const exit = join(temporary, "exit");
  const ansi = join(temporary, "terminal.ansi");
  await Deno.writeTextFile(
    config,
    "set -g remain-on-exit on\nset -g default-terminal xterm-256color\n",
  );
  const script = `/bin/stty -g > ${quote(before)}; env DENO_DIR=${
    quote(cache)
  } ${argv.map(quote).join(" ")}; result=$?; /bin/stty -g > ${
    quote(after)
  }; printf '%s' "$result" > ${quote(exit)}`;
  try {
    // Each case owns its socket/server; never touch an existing development session.
    await tmux(
      "-f",
      config,
      "new-session",
      "-d",
      "-s",
      "demo",
      "-x",
      "80",
      "-y",
      "24",
      "-c",
      temporary,
      script,
    );
    await tmux("pipe-pane", "-t", "demo", `cat > ${quote(ansi)}`);
    await waitScreen((text) =>
      contains(text, "80x24", "Message 60", "Compose · 1 lines", "Ctrl-C: exit")
    );
    const tty =
      (await tmux("display-message", "-p", "-t", "demo", "#{pane_tty}")).trim();
    await until(
      () => run("/bin/stty", ["-f", tty, "-a"]),
      (text) => contains(text, "-icanon", "-echo "),
    );
    await tmux("send-keys", "-t", "demo", "-l", "first line");
    await tmux("send-keys", "-t", "demo", "Enter");
    await tmux("send-keys", "-t", "demo", "-l", "second linX");
    await waitScreen((text) =>
      contains(text, "first line", "second linX", "Compose · 2 lines")
    );
    await tmux("send-keys", "-t", "demo", "BSpace");
    await waitScreen((text) =>
      text.includes("second lin") && !text.includes("second linX")
    );
    await tmux("send-keys", "-t", "demo", "-l", "e");
    await waitScreen((text) =>
      contains(text, "first line", "second line", "Compose · 2 lines")
    );
    await tmux("send-keys", "-t", "demo", "PPage");
    const expected = Array.from(
      { length: 14 },
      (_, i) =>
        `Message ${i + 43} · ${
          i % 2 ? "Coworker" : "You"
        }: Synthetic conversation history.`,
    );
    const older = await waitScreen((text) =>
      contains(text, "older 5", "first line", "second line", ...expected) &&
      !text.includes("Message 60")
    );
    const tick = Number(older.match(/stream (\d+)\//)?.[1]);
    await waitScreen((text) =>
      Number(text.match(/stream (\d+)\//)?.[1]) > Math.max(tick, 80) &&
      contains(text, "older 5", "first line", "second line") &&
      text.split("\n").slice(2, 16).join("\n") ===
        older.split("\n").slice(2, 16).join("\n")
    );
    await tmux("resize-window", "-t", "demo", "-x", "50", "-y", "18");
    await waitScreen((text) =>
      contains(
        text,
        "50x18",
        "older 5",
        "Message 56",
        "first line",
        "second line",
      )
    );
    await tmux("send-keys", "-t", "demo", "NPage");
    await waitScreen((text) =>
      contains(text, "older 0", "Coworker: Streaming", "second line")
    );
    await tmux("send-keys", "-t", "demo", "C-e");
    await waitScreen((text) =>
      contains(
        text,
        "sent 1",
        "You sent 1:",
        "first line",
        "second line",
        "Compose · 1 lines",
        "Type a message",
      )
    );
    await tmux("resize-window", "-t", "demo", "-x", "100", "-y", "32");
    await waitScreen((text) =>
      contains(
        text,
        "100x32",
        "sent 1",
        "You sent 1:",
        "second line",
        "Type a message",
        "Ctrl-C: exit",
      )
    );
    await Deno.writeTextFile(
      join(artifacts, `${name}-screen.txt`),
      await screen(),
    );
    if (termination === "ctrl-c") await tmux("send-keys", "-t", "demo", "C-c");
    else {
      // Signal the demo child, leaving the pane shell to record its terminal state.
      const shellPid =
        (await tmux("display-message", "-p", "-t", "demo", "#{pane_pid}"))
          .trim();
      const pid = Number((await run("pgrep", ["-P", shellPid])).trim());
      assert(Number.isInteger(pid) && pid > 0, "expected one demo process");
      Deno.kill(pid, "SIGTERM");
    }
    const exitCode = await until(async () => {
      try {
        return await Deno.readTextFile(exit);
      } catch (error) {
        if (error instanceof Deno.errors.NotFound) return "";
        throw error;
      }
    }, (text) => text.length > 0);
    assert.equal(Number(exitCode), termination === "ctrl-c" ? 0 : 143);
    assert.equal(
      await Deno.readTextFile(after),
      await Deno.readTextFile(before),
      "terminal attributes restored exactly",
    );
    await until(
      () => Deno.readTextFile(ansi),
      (text) => text.includes("\x1b[?25h"),
    );
    await Deno.copyFile(ansi, join(artifacts, `${name}-terminal.ansi`));
    console.log(
      `${name}: real tmux PTY input/history/stream/resize/exit/restoration pass`,
    );
  } finally {
    await tmux("kill-server").catch(() => {});
    await Deno.remove(temporary, { recursive: true });
  }
}

Deno.test("source rejects non-TTY input with a clear diagnostic", async () => {
  await rejectsNonTty(Deno.execPath(), [
    "run",
    "--frozen",
    "--config",
    join(root, "deno.json"),
    "--allow-env",
    "--allow-read",
    join(root, "main.tsx"),
  ]);
});

Deno.test({
  name: "source conversation in a real tmux PTY",
  ignore: Deno.build.os !== "darwin",
  fn: async () => {
    await Deno.mkdir(artifacts, { recursive: true });
    await terminalCase("source", [
      Deno.execPath(),
      "run",
      "--frozen",
      "--config",
      join(root, "deno.json"),
      "--allow-env",
      "--allow-read",
      join(root, "main.tsx"),
    ]);
  },
});

Deno.test({
  name: "compiled conversation without source in real tmux PTYs",
  ignore: Deno.build.os !== "darwin" || Deno.build.arch !== "aarch64",
  fn: async () => {
    await Deno.mkdir(artifacts, { recursive: true });
    const temporary = await Deno.makeTempDir({
      dir: artifacts,
      prefix: "packaging-",
    });
    const source = join(temporary, "source");
    const binary = join(temporary, "ink-deno");
    try {
      await Deno.mkdir(source);
      for (const name of ["main.tsx", "deno.json", "deno.lock"]) {
        await Deno.copyFile(join(root, name), join(source, name));
      }
      const compile = await run(Deno.execPath(), [
        "compile",
        "--frozen",
        "--config",
        join(source, "deno.json"),
        "--allow-env",
        "--allow-read",
        "--target",
        "aarch64-apple-darwin",
        "--output",
        binary,
        join(source, "main.tsx"),
      ], source);
      await Deno.writeTextFile(
        join(artifacts, "packaging-compile.log"),
        compile,
      );
      await Deno.remove(source, { recursive: true });
      await rejectsNonTty(binary, []);
      await terminalCase("compiled", [binary]);
      await terminalCase("compiled-sigterm", [binary], "sigterm");
    } finally {
      await Deno.remove(temporary, { recursive: true });
    }
  },
});
