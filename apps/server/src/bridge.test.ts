import { strict as assert } from "node:assert";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { WorkspaceState } from "@socium/client/protocol";
import { createBridge } from "./bridge.ts";

/** Stands in for `socium --client-bridge`: one session, then a scripted turn. */
const worker = `#!/usr/bin/env node
const { createInterface } = require("node:readline");
const emit = (event) => process.stdout.write(JSON.stringify(event) + "\\n");
const session = (changes) => emit({
  type: "session", session: "s1", title: null, model: "local", sessions: [], messages: [], warnings: [],
  models: [{ alias: "local", provider: "local", description: null, unavailable: null }], plan: null, ...changes,
});
const plan = { goal: "Fix", revision: 1, done: 500, tasks: [{ id: "a", depth: 0, title: "A", status: "active", group: false, note: "" }] };
session({});
createInterface({ input: process.stdin }).on("line", (line) => {
  const { id, action } = JSON.parse(line);
  if (action === "prompt") {
    emit({ type: "running", turn: id, text: "Go." });
    emit({ type: "response", id, error: null });
    emit({ type: "activity", phase: "tool", detail: "run_command" });
    emit({ type: "plan", plan });
    emit({ type: "approval", request: 3, tool: "run_command", summary: "cargo test", input: "", requestedBy: null, sessionOption: "\`cargo test\` commands" });
  } else if (action === "approve") {
    emit({ type: "approval_resolved", request: 3 });
    emit({ type: "response", id, error: null });
    emit({ type: "outcome", notices: ["Done."] });
    session({ plan });
  }
});
`;

test("the bridge tells subscribers about each change and when the worker stops", { timeout: 10_000 }, async (context) => {
  const directory = mkdtempSync(join(tmpdir(), "socium-bridge-"));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, "socium");
  writeFileSync(binary, worker);
  chmodSync(binary, 0o755);
  const bridge = createBridge(binary, directory, { generation: 7 });
  context.after(() => bridge.close());

  const seen: (WorkspaceState | null)[] = [];
  const unsubscribe = bridge.subscribe((state) => seen.push(state));
  const next = () => new Promise<WorkspaceState | null>((resolve) => {
    const stop = bridge.subscribe((state) => {
      stop();
      resolve(state);
    });
  });
  const initial = await next();
  assert.equal(initial?.models[0]?.alias, "local");
  assert.equal(initial?.generation, 7);
  assert.equal(initial?.plan, null);

  const running = await bridge.send({ action: "prompt", text: "Go." });
  assert.deepEqual(running.turn?.messages, [{ role: "user", text: "Go." }]);
  await new Promise((resolve) => setTimeout(resolve, 100));
  const waiting = bridge.snapshot();
  assert.deepEqual(waiting.turn?.activity, { phase: "tool", detail: "run_command" });
  assert.equal(waiting.plan?.tasks[0]?.status, "active");
  assert.equal(waiting.turn?.approval?.sessionOption, "`cargo test` commands");

  const finished = await bridge.send({ action: "approve", request: 3, allow: true, scope: "session" });
  assert.equal(finished.turn, null);
  assert.equal(finished.plan?.goal, "Fix");
  assert.deepEqual(finished.notices, ["Done."]);
  // One call per change, each with a new revision.
  const revisions = seen.map((state) => state?.revision);
  assert.equal(new Set(revisions).size, revisions.length);
  assert(seen.every((state) => state?.generation === 7));
  assert(seen.length >= 6, `${seen.length} changes`);
  assert.equal(seen.at(-1), finished);

  unsubscribe();
  const stopped = next();
  const count = seen.length;
  bridge.close();
  assert.equal(await stopped, null);
  assert.equal(seen.length, count);
});
