import { createServer } from "node:http";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = await mkdtemp(join(tmpdir(), "cwr-source-web-"));
const repo = join(root, "repo");
const actors = Object.fromEntries(["alice", "bob"].map((name) => [name, {
  workspace: join(root, name), cloned: false, staged: [], proposal: null, history: [], suggestion: null, lap: 1,
}]));
let initialized = false;
let accepted = [];
let notice = "Create a repository to begin.";
let busy = false;

function cli(...args) {
  const result = spawnSync(process.execPath, [join(here, "cli.mjs"), ...args], {
    encoding: "utf8", timeout: 30_000,
  });
  if (result.status !== 0) throw new Error(result.stderr.trim() || `CLI exited ${result.status}`);
  return JSON.parse(result.stdout.trim().split("\n").at(-1));
}

async function source() {
  if (!initialized) return { version: null, files: {} };
  const workspace = join(root, "viewer");
  await rm(workspace, { recursive: true, force: true });
  const { parentVersion } = cli("clone", repo, "viewer", workspace);
  const files = {};
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue;
      const path = join(directory, entry.name);
      if (entry.isDirectory()) await walk(path);
      else files[path.slice(workspace.length + 1)] = await readFile(path, "utf8");
    }
  }
  await walk(workspace);
  return { version: parentVersion, files };
}

async function state() {
  return {
    initialized, actors: Object.fromEntries(Object.entries(actors).map(([name, actor]) => [name, {
      cloned: actor.cloned, staged: actor.staged, proposal: actor.proposal && {
        status: actor.proposal.status, changedPaths: actor.proposal.changedPaths,
        parentVersion: actor.proposal.parentVersion, review: actor.proposal.review, ci: actor.proposal.ci, paths: actor.proposal.paths,
      }, history: actor.history, suggestion: actor.suggestion, lap: actor.lap,
    }])), accepted, notice, source: await source(),
  };
}

function actorFor(name) {
  if (!(name in actors)) throw new Error("Choose Alice or Bob.");
  return actors[name];
}

async function act(input) {
  const { action, name } = input;
  if (action === "init") {
    if (initialized) throw new Error("Repository already exists.");
    cli("init", repo);
    initialized = true;
    notice = "Owner created the parent Hyperdrive and its first checkpoint.";
  } else if (action === "clone") {
    if (!initialized) throw new Error("Create the repository first.");
    const actor = actorFor(name);
    if (actor.cloned) throw new Error(`${name} already has a clone.`);
    const result = cli("clone", repo, name, actor.workspace);
    actor.cloned = true;
    actor.parentVersion = result.parentVersion;
    notice = `${name} cloned parent version ${result.parentVersion} with a separate keypair.`;
  } else if (action === "stage") {
    const actor = actorFor(name);
    if (!actor.cloned || actor.proposal) throw new Error("Start or revise a clone before editing.");
    const filename = String(input.filename ?? "").trim();
    const content = String(input.content ?? "");
    if (!filename || filename.startsWith(".") || filename.includes("\\") || filename.split("/").includes("..")) throw new Error("Use a simple relative filename.");
    const path = resolve(actor.workspace, filename);
    if (!path.startsWith(actor.workspace + sep)) throw new Error("Filename leaves the workspace.");
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, content);
    if (!actor.staged.includes(filename)) actor.staged.push(filename);
    actor.suggestion = { filename, content };
    notice = `${name} edited ${filename} in a local workspace. The parent is unchanged.`;
  } else if (action === "submit") {
    const actor = actorFor(name);
    if (!actor.cloned || actor.proposal || !actor.staged.length) throw new Error("Stage an edit in a clone first.");
    const result = cli("submit", repo, name, actor.workspace);
    actor.proposal = { ...result, parentVersion: actor.parentVersion, status: "waiting" };
    notice = `${name} signed a proposal for parent version ${actor.parentVersion}. The parent is unchanged.`;
  } else if (action === "review") {
    const actor = actorFor(name);
    if (!actor.proposal || actor.proposal.status !== "waiting" || actor.proposal.review) throw new Error("This proposal has already been reviewed; review its revision instead.");
    const decision = input.decision;
    const feedback = String(input.feedback ?? "").trim();
    if (decision === "changes_requested" && !feedback) throw new Error("Give the contributor feedback before requesting changes.");
    const result = cli("review", repo, actor.proposal.manifest, decision, feedback);
    actor.proposal.review = result;
    notice = decision === "approve"
      ? `Reviewer approved ${name}'s exact proposal checkpoint. The parent is unchanged.`
      : `Reviewer asked ${name} for changes: ${feedback}. Integration is blocked.`;
  } else if (action === "ci") {
    const actor = actorFor(name);
    if (!actor.proposal || actor.proposal.status !== "waiting" || actor.proposal.ci) throw new Error("CI needs a new proposal checkpoint.");
    const result = cli("ci", repo, actor.proposal.manifest);
    actor.proposal.ci = result;
    notice = result.status === "passed"
      ? `CI passed for ${name}'s exact proposal checkpoint.`
      : `CI failed for ${name}'s proposal: ${result.failures.join(", ")}. Revise and submit a new proposal.`;
  } else if (action === "revise") {
    const actor = actorFor(name);
    const previous = actor.proposal;
    if (!previous || (previous.review?.status !== "changes_requested" && previous.ci?.status !== "failed" && previous.status !== "conflict")) throw new Error("Only a returned proposal can be revised.");
    const filename = previous.changedPaths[0];
    let content = "";
    try { content = await readFile(join(actor.workspace, filename), "utf8"); } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    actor.history.push({ status: previous.status, review: previous.review, ci: previous.ci, changedPaths: previous.changedPaths, parentVersion: previous.parentVersion });
    if (previous.status === "conflict") {
      actor.workspace = join(root, `${name}-revision-${actor.history.length}`);
      const result = cli("clone", repo, name, actor.workspace);
      actor.parentVersion = result.parentVersion;
      notice = `${name} started a revision from current parent version ${result.parentVersion}. Compare the accepted file, then save a revised edit.`;
    } else {
      notice = `${name} can edit the workspace and submit a new proposal for review and CI.`;
    }
    actor.proposal = null;
    actor.staged = [];
    actor.suggestion = { filename, content };
  } else if (action === "next_lap") {
    const actor = actorFor(name);
    if (actor.proposal?.status !== "accepted") throw new Error("Finish integration before starting the next lap.");
    const previous = actor.proposal;
    const nextLap = actor.lap + 1;
    const nextWorkspace = join(root, `${name}-lap-${nextLap}`);
    const result = cli("clone", repo, name, nextWorkspace);
    actor.history.push({ status: previous.status, review: previous.review, ci: previous.ci, changedPaths: previous.changedPaths, parentVersion: previous.parentVersion });
    actor.workspace = nextWorkspace;
    actor.parentVersion = result.parentVersion;
    actor.lap = nextLap;
    actor.proposal = null;
    actor.staged = [];
    actor.suggestion = { filename: `${name}-lap-${nextLap}.txt`, content: `A change from ${name} in lap ${nextLap}` };
    notice = `${name} started lap ${nextLap} from accepted parent version ${result.parentVersion}. Save an edit to make a new proposal.`;
  } else if (action === "integrate") {
    const actor = actorFor(name);
    if (!actor.proposal || actor.proposal.status !== "waiting" || actor.proposal.review?.status !== "approve" || actor.proposal.ci?.status !== "passed") throw new Error("Reviewer approval and passing CI are required before integration.");
    const result = cli("integrate", repo, actor.proposal.manifest);
    actor.proposal.status = result.status;
    actor.proposal.paths = result.paths;
    if (result.status === "accepted") accepted.push({ actor: name, version: result.parentVersion, paths: actor.proposal.changedPaths });
    notice = result.status === "conflict"
      ? `Integrator returned ${name}'s proposal: ${result.paths.join(", ")} changed since the clone.`
      : `Integrator accepted ${name}'s proposal into parent version ${result.parentVersion}.`;
  } else throw new Error("Unknown action.");
  return state();
}

const server = createServer(async (request, response) => {
  try {
    if (request.method === "GET" && request.url === "/") {
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end(await readFile(join(here, "web.html")));
      return;
    }
    if (request.method === "GET" && request.url === "/api/state") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(await state()));
      return;
    }
    if (request.method === "POST" && request.url === "/api/action") {
      if (busy) throw new Error("Wait for the current action to finish.");
      busy = true;
      try {
        let body = "";
        for await (const chunk of request) {
          body += chunk;
          if (body.length > 10_000) throw new Error("Request too large.");
        }
        const result = await act(JSON.parse(body));
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(result));
      } finally { busy = false; }
      return;
    }
    response.writeHead(404).end();
  } catch (error) {
    response.writeHead(400, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: error.message }));
  }
});

const port = Number(process.env.PORT || 8765);
server.listen(port, "127.0.0.1", () => {
  console.log(`CWR source demo: http://127.0.0.1:${port}`);
  console.log(`Temporary data: ${root}`);
});
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, async () => {
  server.close();
  await rm(root, { recursive: true, force: true });
  process.exit();
});
