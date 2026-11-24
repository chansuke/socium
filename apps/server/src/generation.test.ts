import { strict as assert } from "node:assert";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { nextGeneration } from "./generation.ts";

test("each gateway start takes a larger generation, kept across restarts in a private file", (context) => {
  const directory = mkdtempSync(join(tmpdir(), "socium-generation-"));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, "state", "gateway-generation.sqlite");
  assert.deepEqual([nextGeneration(path), nextGeneration(path), nextGeneration(path)], [1, 2, 3]);
  assert.equal(statSync(path).mode & 0o777, 0o600);
  assert.equal(statSync(join(directory, "state")).mode & 0o777, 0o700);
});
