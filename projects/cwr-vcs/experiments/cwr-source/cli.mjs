import Corestore from "corestore";
import Hyperdrive from "hyperdrive";
import { createHash, generateKeyPairSync, randomUUID, sign, verify } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";

const [command, repo, actor, workspace, feedback = ""] = process.argv.slice(2);
const metadata = ".cwr-source.json";
const json = (value) => JSON.stringify(value, null, 2);
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const fileHash = (files, path) => path in files ? hash(files[path]) : null;
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const saveJson = async (path, value) => writeFile(path, json(value) + "\n");

async function files(root, directory = root) {
  const result = {};
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) Object.assign(result, await files(root, path));
    else if (entry.isFile()) result[relative(root, path)] = await readFile(path);
  }
  return result;
}

async function openDrive(storage, key) {
  const store = new Corestore(storage);
  await store.ready();
  const drive = new Hyperdrive(store, key && Buffer.from(key, "hex"));
  await drive.ready();
  return { store, drive };
}

async function snapshot(drive) {
  const result = {};
  for await (const entry of drive.list("/")) {
    result[entry.key.slice(1)] = await drive.get(entry.key);
  }
  return result;
}

async function identity(repo, actor) {
  const path = join(repo, "actors", `${actor}.json`);
  try { return await readJson(path); } catch (error) {
    if (error.code !== "ENOENT") throw error;
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const keys = {
      publicKey: publicKey.export({ type: "spki", format: "pem" }),
      privateKey: privateKey.export({ type: "pkcs8", format: "pem" }),
    };
    await mkdir(join(repo, "actors"), { recursive: true });
    await saveJson(path, keys);
    return keys;
  }
}

async function parent(repo) {
  const state = await readJson(join(repo, "repository.json"));
  const opened = await openDrive(join(repo, "parent"), state.key);
  return { ...opened, state };
}

async function init() {
  await mkdir(repo, { recursive: true });
  const { store, drive } = await openDrive(join(repo, "parent"));
  await drive.put("/README.md", Buffer.from("# Example repository\n"));
  const integratorKeys = await identity(repo, "integrator");
  const ciKeys = await identity(repo, "ci");
  const state = { key: drive.key.toString("hex"), owner: "owner", integratorKey: integratorKeys.publicKey, ciKey: ciKeys.publicKey };
  await saveJson(join(repo, "repository.json"), state);
  await identity(repo, "owner");
  await store.close();
  return { status: "initialized", ...state };
}

async function clone() {
  const { store, drive, state } = await parent(repo);
  const version = drive.version;
  const source = await snapshot(drive.checkout(version));
  await mkdir(workspace, { recursive: true });
  for (const [path, bytes] of Object.entries(source)) {
    await mkdir(join(workspace, path, ".."), { recursive: true });
    await writeFile(join(workspace, path), bytes);
  }
  await saveJson(join(workspace, metadata), {
    parentKey: state.key, parentVersion: version,
    baseHashes: Object.fromEntries(Object.entries(source).map(([path, bytes]) => [path, hash(bytes)])),
  });
  await identity(repo, actor);
  await store.close();
  return { status: "cloned", actor, parentVersion: version };
}

async function submit() {
  const base = await readJson(join(workspace, metadata));
  const source = await files(workspace);
  const paths = [...new Set([...Object.keys(base.baseHashes), ...Object.keys(source)])].sort();
  const changedPaths = paths.filter((path) => base.baseHashes[path] !== (source[path] && hash(source[path])));
  if (!changedPaths.length) throw new Error("workspace has no changes");
  const proposalId = `${actor}-${Date.now()}-${randomUUID()}`;
  const location = join(repo, "proposals", proposalId);
  await mkdir(location, { recursive: true });
  const { store, drive } = await openDrive(join(location, "drive"));
  for (const [path, bytes] of Object.entries(source)) await drive.put("/" + path, bytes);
  const keys = await identity(repo, actor);
  const manifest = {
    actor, parentKey: base.parentKey, parentVersion: base.parentVersion,
    proposalKey: drive.key.toString("hex"), proposalVersion: drive.version,
    changedPaths,
    sourceHashes: Object.fromEntries(Object.entries(source).map(([path, bytes]) => [path, hash(bytes)])),
    publicKey: keys.publicKey,
  };
  const signature = sign(null, Buffer.from(JSON.stringify(manifest)), keys.privateKey).toString("base64");
  const manifestPath = join(location, "manifest.json");
  await saveJson(manifestPath, { manifest, signature });
  await store.close();
  return { status: "submitted", manifest: manifestPath, changedPaths };
}

async function review() {
  const manifestPath = actor;
  const decision = workspace;
  if (!["approve", "changes_requested"].includes(decision)) throw new Error("review decision must be approve or changes_requested");
  const reviewPath = join(dirname(manifestPath), "review.json");
  try {
    await readFile(reviewPath);
    throw new Error("proposal already reviewed; submit a new proposal for another review");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const { manifest } = await readJson(manifestPath);
  const keys = await identity(repo, "reviewer");
  const record = {
    reviewer: "reviewer", publicKey: keys.publicKey, decision, feedback,
    parentKey: manifest.parentKey, parentVersion: manifest.parentVersion,
    proposalKey: manifest.proposalKey, proposalVersion: manifest.proposalVersion,
  };
  const signature = sign(null, Buffer.from(JSON.stringify(record)), keys.privateKey).toString("base64");
  await saveJson(reviewPath, { record, signature });
  return { status: decision, reviewer: "reviewer", feedback };
}

async function ci() {
  const manifestPath = actor;
  const ciPath = join(dirname(manifestPath), "ci.json");
  try {
    await readFile(ciPath);
    throw new Error("CI already ran for this proposal; submit a new proposal to rerun it");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const { manifest, signature } = await readJson(manifestPath);
  const contributorKeys = await readJson(join(repo, "actors", `${manifest.actor}.json`));
  if (manifest.publicKey !== contributorKeys.publicKey ||
      !verify(null, Buffer.from(JSON.stringify(manifest)), contributorKeys.publicKey, Buffer.from(signature, "base64"))) {
    throw new Error("invalid contributor signature");
  }
  const { store, drive } = await openDrive(join(dirname(manifestPath), "drive"), manifest.proposalKey);
  try {
    const proposed = await snapshot(drive.checkout(manifest.proposalVersion));
    const hashes = Object.fromEntries(Object.entries(proposed).map(([path, bytes]) => [path, hash(bytes)]));
    if (JSON.stringify(hashes) !== JSON.stringify(manifest.sourceHashes)) throw new Error("proposal checkpoint changed");
    const failures = manifest.changedPaths.filter((path) => {
      const bytes = proposed[path];
      if (!bytes) return false; // Deleting a file is allowed.
      const content = bytes.toString("utf8");
      return !content.trim() || /^(<<<<<<<|=======|>>>>>>>)/m.test(content);
    });
    const keys = await identity(repo, "ci");
    const record = {
      actor: "ci", publicKey: keys.publicKey, check: "source-sanity-v1",
      passed: failures.length === 0, failures,
      parentKey: manifest.parentKey, parentVersion: manifest.parentVersion,
      proposalKey: manifest.proposalKey, proposalVersion: manifest.proposalVersion,
    };
    const ciSignature = sign(null, Buffer.from(JSON.stringify(record)), keys.privateKey).toString("base64");
    await saveJson(ciPath, { record, signature: ciSignature });
    return { status: record.passed ? "passed" : "failed", check: record.check, failures };
  } finally {
    await store.close();
  }
}

async function integrate() {
  const manifestPath = actor;
  const { manifest, signature } = await readJson(manifestPath);
  const { record: reviewRecord, signature: reviewSignature } = await readJson(join(dirname(manifestPath), "review.json"));
  const reviewerKeys = await readJson(join(repo, "actors", "reviewer.json"));
  if (reviewRecord.reviewer !== "reviewer" || reviewRecord.publicKey !== reviewerKeys.publicKey ||
      !verify(null, Buffer.from(JSON.stringify(reviewRecord)), reviewerKeys.publicKey, Buffer.from(reviewSignature, "base64")) ||
      reviewRecord.parentKey !== manifest.parentKey || reviewRecord.parentVersion !== manifest.parentVersion ||
      reviewRecord.proposalKey !== manifest.proposalKey || reviewRecord.proposalVersion !== manifest.proposalVersion) {
    throw new Error("invalid review for this proposal checkpoint");
  }
  if (reviewRecord.decision !== "approve") throw new Error("reviewer requested changes; integration requires approval");
  const { record: ciRecord, signature: ciSignature } = await readJson(join(dirname(manifestPath), "ci.json"));
  const repoState = await readJson(join(repo, "repository.json"));
  if (ciRecord.actor !== "ci" || ciRecord.publicKey !== repoState.ciKey || ciRecord.check !== "source-sanity-v1" ||
      !verify(null, Buffer.from(JSON.stringify(ciRecord)), repoState.ciKey, Buffer.from(ciSignature, "base64")) ||
      ciRecord.parentKey !== manifest.parentKey || ciRecord.parentVersion !== manifest.parentVersion ||
      ciRecord.proposalKey !== manifest.proposalKey || ciRecord.proposalVersion !== manifest.proposalVersion) {
    throw new Error("invalid CI result for this proposal checkpoint");
  }
  if (!ciRecord.passed) throw new Error(`CI failed: ${ciRecord.failures.join(", ")}`);
  const keys = await readJson(join(repo, "actors", `${manifest.actor}.json`));
  if (keys.publicKey !== manifest.publicKey || !verify(null, Buffer.from(JSON.stringify(manifest)), keys.publicKey, Buffer.from(signature, "base64"))) {
    throw new Error("invalid contributor signature");
  }
  const { store, drive, state } = await parent(repo);
  const integratorKeys = await readJson(join(repo, "actors", "integrator.json"));
  if (integratorKeys.publicKey !== state.integratorKey) throw new Error("integrator key is not authorized for this parent");
  if (manifest.parentKey !== state.key || manifest.parentVersion > drive.version) throw new Error("invalid parent checkpoint");
  const base = await snapshot(drive.checkout(manifest.parentVersion));
  const current = await snapshot(drive.checkout(drive.version));
  const proposalLocation = dirname(manifestPath);
  const proposal = await openDrive(join(proposalLocation, "drive"), manifest.proposalKey);
  const proposed = await snapshot(proposal.drive.checkout(manifest.proposalVersion));
  const actualHashes = Object.fromEntries(Object.entries(proposed).map(([path, bytes]) => [path, hash(bytes)]));
  if (JSON.stringify(actualHashes) !== JSON.stringify(manifest.sourceHashes)) throw new Error("proposal checkpoint changed");
  const actualChanged = [...new Set([...Object.keys(base), ...Object.keys(proposed)])].sort().filter((path) => fileHash(base, path) !== fileHash(proposed, path));
  if (JSON.stringify(actualChanged) !== JSON.stringify(manifest.changedPaths)) throw new Error("manifest paths do not match proposal");
  const conflicts = manifest.changedPaths.filter((path) => fileHash(base, path) !== fileHash(current, path));
  if (conflicts.length) {
    await proposal.store.close(); await store.close();
    return { status: "conflict", paths: conflicts };
  }
  for (const path of manifest.changedPaths) {
    if (proposed[path]) await drive.put("/" + path, proposed[path]);
    else await drive.del("/" + path);
  }
  const result = { status: "accepted", actor: manifest.actor, parentVersion: drive.version };
  const integration = { manifestPath, review: reviewRecord, ci: ciRecord, ...result };
  const integrationSignature = sign(null, Buffer.from(JSON.stringify(integration)), integratorKeys.privateKey).toString("base64");
  await mkdir(join(repo, "integrations"), { recursive: true });
  await saveJson(join(repo, "integrations", `${drive.version}.json`), { integration, integratorKey: state.integratorKey, signature: integrationSignature });
  await proposal.store.close(); await store.close();
  return result;
}

try {
  if (!repo) throw new Error("usage: cli.mjs <init|clone|submit|review|ci|integrate> <repoDir> [actor] [workspaceDir]");
  const result = await ({ init, clone, submit, review, ci, integrate })[command]?.();
  if (!result) throw new Error(`unknown command: ${command}`);
  console.log(JSON.stringify(result));
} catch (error) {
  console.error(error.stack ?? String(error));
  process.exitCode = 1;
}
