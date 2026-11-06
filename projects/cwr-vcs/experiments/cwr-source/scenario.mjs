import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";

const experimentDir = dirname(fileURLToPath(import.meta.url));
const cliPath = join(experimentDir, "cli.mjs");
async function command(...args) {
  const result = spawnSync(process.execPath, [cliPath, ...args], { encoding: "utf8" });
  const stdout = result.stdout.trim();
  const stderr = result.stderr.trim();
  if (result.status !== 0) {
    throw new Error(
      `cli ${args[0]} failed (${result.status})\n${stdout}\n${stderr}`,
    );
  }
  const records = stdout.split("\n").filter(Boolean).map((line) =>
    JSON.parse(line)
  );
  if (records.length === 0) throw new Error(`cli ${args[0]} returned no JSON`);
  return records.at(-1);
}

function expect(value, message) {
  if (!value) throw new Error(message);
}

export async function runScenario() {
  const root = await mkdtemp(join(tmpdir(), "cwr-source-scenario-"));
  try {
    const repo = join(root, "repo");
    const alice = join(root, "alice");
    const bob = join(root, "bob");
    const fresh = join(root, "fresh");

    await command("init", repo);
    await command("clone", repo, "alice", alice);
    await command("clone", repo, "bob", bob);
    await writeFile(join(alice, "alice.txt"), "from alice\n");
    await writeFile(join(bob, "bob.txt"), "from bob\n");

    const aliceProposal = await command("submit", repo, "alice", alice);
    const bobProposal = await command("submit", repo, "bob", bob);
    expect(
      aliceProposal.manifest,
      "Alice submission did not return a manifest path",
    );
    expect(
      bobProposal.manifest,
      "Bob submission did not return a manifest path",
    );

    await command("review", repo, aliceProposal.manifest, "approve", "Looks good");
    const missingCi = spawnSync(process.execPath, [cliPath, "integrate", repo, aliceProposal.manifest], { encoding: "utf8" });
    expect(missingCi.status !== 0, "Missing CI did not block integration");
    await command("ci", repo, aliceProposal.manifest);
    await command("review", repo, bobProposal.manifest, "changes_requested", "Please explain this file");
    const blocked = spawnSync(process.execPath, [cliPath, "integrate", repo, bobProposal.manifest], { encoding: "utf8" });
    expect(blocked.status !== 0 && blocked.stderr.includes("reviewer requested changes"), "Feedback did not block integration");
    await writeFile(join(bob, "bob.txt"), "from bob, with explanation\n");
    const bobRevision = await command("submit", repo, "bob", bob);
    expect(bobRevision.manifest !== bobProposal.manifest, "Revision reused the returned proposal");
    await command("review", repo, bobRevision.manifest, "approve", "Now clear");
    await command("ci", repo, bobRevision.manifest);

    const first = await command("integrate", repo, aliceProposal.manifest);
    const second = await command("integrate", repo, bobRevision.manifest);
    expect(
      first.status === "accepted",
      `Alice integration: ${JSON.stringify(first)}`,
    );
    expect(
      second.status === "accepted",
      `Bob integration: ${JSON.stringify(second)}`,
    );

    await command("clone", repo, "fresh", fresh);
    expect(
      await readFile(join(fresh, "alice.txt"), "utf8") === "from alice\n",
      "Fresh clone lost Alice's change",
    );
    expect(
      await readFile(join(fresh, "bob.txt"), "utf8") === "from bob, with explanation\n",
      "Fresh clone lost Bob's change",
    );

    const aliceSecondLap = join(root, "alice-second-lap");
    const secondLapClone = await command("clone", repo, "alice", aliceSecondLap);
    expect(secondLapClone.parentVersion === second.parentVersion, "Second lap did not start from the accepted parent");
    await writeFile(join(aliceSecondLap, "alice-second.txt"), "another lap\n");
    const secondLapProposal = await command("submit", repo, "alice", aliceSecondLap);
    await command("review", repo, secondLapProposal.manifest, "approve", "Looks good");
    await command("ci", repo, secondLapProposal.manifest);
    const secondLapResult = await command("integrate", repo, secondLapProposal.manifest);
    expect(secondLapResult.status === "accepted", "Second lap was not accepted");

    const carol = join(root, "carol");
    const dave = join(root, "dave");
    await command("clone", repo, "carol", carol);
    await command("clone", repo, "dave", dave);
    await writeFile(join(carol, "shared.txt"), "from carol\n");
    await writeFile(join(dave, "shared.txt"), "from dave\n");
    const carolProposal = await command("submit", repo, "carol", carol);
    const daveProposal = await command("submit", repo, "dave", dave);
    await command("review", repo, carolProposal.manifest, "approve", "Looks good");
    await command("review", repo, daveProposal.manifest, "approve", "Looks good");
    await command("ci", repo, carolProposal.manifest);
    await command("ci", repo, daveProposal.manifest);
    const third = await command("integrate", repo, carolProposal.manifest);
    const fourth = await command("integrate", repo, daveProposal.manifest);
    expect(
      third.status === "accepted",
      `Carol integration: ${JSON.stringify(third)}`,
    );
    expect(
      fourth.status === "conflict",
      `Overlapping change should conflict: ${JSON.stringify(fourth)}`,
    );

    const afterConflict = join(root, "after-conflict");
    await command("clone", repo, "after-conflict", afterConflict);
    expect(
      await readFile(join(afterConflict, "shared.txt"), "utf8") ===
        "from carol\n",
      "Conflict changed accepted source",
    );

    const daveRevisionWorkspace = join(root, "dave-revision");
    await command("clone", repo, "dave", daveRevisionWorkspace);
    await writeFile(join(daveRevisionWorkspace, "shared.txt"), "from carol and dave\n");
    const daveRevision = await command("submit", repo, "dave", daveRevisionWorkspace);
    await command("review", repo, daveRevision.manifest, "approve", "Conflict resolved");
    await command("ci", repo, daveRevision.manifest);
    const resolved = await command("integrate", repo, daveRevision.manifest);
    expect(resolved.status === "accepted", "Revised conflict was not accepted");
    const resolvedClone = join(root, "resolved");
    await command("clone", repo, "resolved", resolvedClone);
    expect(await readFile(join(resolvedClone, "shared.txt"), "utf8") === "from carol and dave\n", "Revised source was not cloned");

    const eve = join(root, "eve");
    await command("clone", repo, "eve", eve);
    await writeFile(join(eve, "unfinished.txt"), "<<<<<<< unresolved\n");
    const failedProposal = await command("submit", repo, "eve", eve);
    await command("review", repo, failedProposal.manifest, "approve", "Please run CI");
    const failedCi = await command("ci", repo, failedProposal.manifest);
    expect(failedCi.status === "failed", "Unresolved conflict marker passed CI");
    const failedIntegration = spawnSync(process.execPath, [cliPath, "integrate", repo, failedProposal.manifest], { encoding: "utf8" });
    expect(failedIntegration.status !== 0 && failedIntegration.stderr.includes("CI failed"), "Failed CI did not block integration");
    await writeFile(join(eve, "unfinished.txt"), "resolved\n");
    const fixedProposal = await command("submit", repo, "eve", eve);
    await command("review", repo, fixedProposal.manifest, "approve", "Looks good");
    const passedCi = await command("ci", repo, fixedProposal.manifest);
    expect(passedCi.status === "passed", "Revised proposal did not pass CI");
    const fixed = await command("integrate", repo, fixedProposal.manifest);
    expect(fixed.status === "accepted", "Proposal revised after CI failure was not accepted");

    return {
      disjoint: [first.status, second.status],
      overlapping: [third.status, fourth.status],
      revisions: [second.status, resolved.status],
      secondLap: secondLapResult.status,
      freshClone: true,
      ci: [failedCi.status, passedCi.status],
    };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  console.log(JSON.stringify(await runScenario()));
}
