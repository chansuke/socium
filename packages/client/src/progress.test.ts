import { strict as assert } from "node:assert";
import { test } from "node:test";
import { describeActivity, planProgress } from "./progress.ts";
import type { Plan } from "./protocol.ts";

test("activity reads as one status line", () => {
  assert.equal(describeActivity(null), "Running…");
  assert.equal(describeActivity({ phase: "requesting", detail: "anthropic" }), "Waiting for anthropic…");
  assert.equal(describeActivity({ phase: "command", detail: "cargo" }), "Running cargo…");
  assert.equal(describeActivity({ phase: "command", detail: null }), "Running a command…");
  assert.equal(describeActivity({ phase: "tool", detail: "read_file" }), "Using read_file…");
  assert.equal(describeActivity({ phase: "approval", detail: "run_command" }), "Waiting for approval…");
});

test("plan progress counts leaf tasks and leaves dropped ones out", () => {
  const task = (id: string, status: Plan["tasks"][number]["status"], group = false) =>
    ({ id, depth: group ? 0 : 1, title: id, status, group, note: "" });
  const plan: Plan = {
    goal: "Fix the parser",
    revision: 2,
    done: 333,
    tasks: [task("group", "active", true), task("repro", "done"), task("fix", "active"), task("docs", "pending"), task("old", "dropped")],
  };
  const progress = planProgress(plan);
  assert.deepEqual({ ...progress, active: progress.active?.id }, { done: 1, total: 3, active: "fix", percent: 33 });
});
