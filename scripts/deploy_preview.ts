// Deploy only committed, checked source to the preview-only Worker.
async function git(...args: string[]): Promise<string> {
  const result = await new Deno.Command("git", {
    args,
    stdout: "piped",
    stderr: "inherit",
  }).output();
  if (!result.success) throw new Error(`git ${args[0]} failed`);
  return new TextDecoder().decode(result.stdout).trim();
}
if (await git("status", "--porcelain")) {
  throw new Error("Commit or stash changes before deploying.");
}
const revision = await git("rev-parse", "HEAD");
const checks = await new Deno.Command("deno", {
  args: ["task", "check"],
  stdout: "inherit",
  stderr: "inherit",
}).output();
if (!checks.success) {
  throw new Error("Checks failed; preview was not deployed.");
}
const deployment = await new Deno.Command("npx", {
  args: [
    "--yes",
    "wrangler@4.157.0",
    "deploy",
    "--var",
    `SOURCE_REVISION:${revision}`,
  ],
  stdout: "inherit",
  stderr: "inherit",
}).output();
Deno.exit(deployment.code);
