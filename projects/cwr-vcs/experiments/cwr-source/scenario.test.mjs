import { runScenario } from "./scenario.mjs";
import test from "node:test";

test("independent contributors submit and integrate disjoint source, while overlap conflicts", async () => {
  const result = await runScenario();
  if (
    JSON.stringify(result.disjoint) !== JSON.stringify(["accepted", "accepted"])
  ) {
    throw new Error(`Unexpected disjoint result: ${JSON.stringify(result)}`);
  }
  if (
    JSON.stringify(result.overlapping) !==
      JSON.stringify(["accepted", "conflict"])
  ) {
    throw new Error(`Unexpected overlap result: ${JSON.stringify(result)}`);
  }
  if (!result.freshClone) throw new Error("Fresh clone was not verified");
  if (JSON.stringify(result.revisions) !== JSON.stringify(["accepted", "accepted"])) {
    throw new Error(`Returned proposals were not revised: ${JSON.stringify(result)}`);
  }
  if (result.secondLap !== "accepted") throw new Error("Second lap was not accepted");
});
