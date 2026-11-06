import { useEffect, useState } from "react";
import { Box, render, Text, useApp, useInput, useStdout } from "ink";
import process from "node:process";

// Synthetic only: no host client, credentials, or retained customer state.
const history = Array.from(
  { length: 60 },
  (_, i) =>
    `Message ${String(i + 1).padStart(2, "0")} · ${
      i % 2 ? "Coworker" : "You"
    }: Synthetic conversation history.`,
);
const response =
  "Streaming synthetic reply: Ink renders React components with Yoga layout under Deno. The draft and older history remain available while this response arrives.";

function Demo() {
  const { exit } = useApp();
  const { stdout } = useStdout();
  const [size, setSize] = useState({
    columns: stdout.columns,
    rows: stdout.rows,
  });
  const [draft, setDraft] = useState("");
  const [sent, setSent] = useState<string[]>([]);
  const [offset, setOffset] = useState(0);
  const [stream, setStream] = useState(0);
  useEffect(() => {
    const resize = () =>
      setSize({ columns: stdout.columns, rows: stdout.rows });
    stdout.on("resize", resize);
    const timer = setInterval(
      () => setStream((n) => Math.min(response.length, n + 2)),
      80,
    );
    return () => {
      stdout.off("resize", resize);
      clearInterval(timer);
    };
  }, [stdout]);
  useInput((input, key) => {
    if (key.ctrl && input === "c") return exit();
    if (key.pageUp) {
      return setOffset((n) => Math.min(history.length - 1, n + 5));
    }
    if (key.pageDown) return setOffset((n) => Math.max(0, n - 5));
    if (key.ctrl && input === "e") {
      if (draft.trim()) setSent((messages) => [...messages, draft]);
      setDraft("");
      setOffset(0);
      return;
    }
    if (key.return) return setDraft((text) => text + "\n");
    if (key.backspace || key.delete) {
      return setDraft((text) => Array.from(text).slice(0, -1).join(""));
    }
    if (
      !key.ctrl && !key.meta && !key.escape && !key.upArrow && !key.downArrow &&
      !key.leftArrow && !key.rightArrow
    ) {
      setDraft((text) => text + input.replaceAll("\r", "\n"));
    }
  });
  const columns = Math.max(12, size.columns || 80);
  const rows = Math.max(8, size.rows || 24);
  const composerHeight = Math.min(5, Math.max(2, Math.floor(rows / 4)));
  const viewportHeight = Math.max(1, rows - composerHeight - 5);
  const messages = [
    ...history,
    ...sent.flatMap((text, i) => [`You sent ${i + 1}:`, ...text.split("\n")]),
    `Coworker: ${response.slice(0, stream)}`,
  ];
  const lines = messages.slice(0, Math.max(1, messages.length - offset))
    .flatMap((text) => {
      const chars = Array.from(text);
      return Array.from({
        length: Math.max(1, Math.ceil(chars.length / columns)),
      }, (_, i) => chars.slice(i * columns, (i + 1) * columns).join(""));
    });
  const end = lines.length;
  return (
    <Box flexDirection="column" width={columns} height={rows - 1}>
      <Text bold color="cyan" wrap="truncate">
        Ink / Deno · synthetic conversation
      </Text>
      <Text wrap="truncate">
        {`${columns}x${rows} · older ${offset} · stream ${stream}/${response.length} · sent ${sent.length}`}
      </Text>
      <Box flexDirection="column" height={viewportHeight} overflow="hidden">
        {lines.slice(Math.max(0, end - viewportHeight), end).map((line, i) => (
          <Text key={i}>{line}</Text>
        ))}
      </Box>
      <Text color="green">Compose · {draft.split("\n").length} lines</Text>
      <Box height={composerHeight} flexDirection="column" overflow="hidden">
        <Text>{draft || "Type a message…"}</Text>
      </Box>
      <Text dimColor wrap="truncate">
        Enter: newline · Ctrl-E: send · PgUp/PgDn: history · Ctrl-C: exit
      </Text>
    </Box>
  );
}

if (!process.stdin.isTTY || !process.stdout.isTTY) {
  console.error("This synthetic demo needs an interactive terminal.");
  Deno.exit(1);
}
const app = render(<Demo />, { exitOnCtrlC: false, patchConsole: false });
await app.waitUntilExit();
