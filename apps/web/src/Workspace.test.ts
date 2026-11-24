import { strict as assert } from "node:assert";
import { after, test } from "node:test";
import type { Client } from "@socium/client";
import type { Plan, WorkspaceState } from "@socium/client/protocol";
import { elements, FakeEvent, installFakeDom } from "./testing/fakeDom.ts";
import type { FakeElement } from "./testing/fakeDom.ts";

// react-dom checks for a DOM once, when it loads, so the fake one comes first.
const dom = installFakeDom();
const globals = globalThis as Record<string, unknown>;
const saved = { matchMedia: globals.matchMedia, ResizeObserver: globals.ResizeObserver };
// The plan panel asks for the screen width, and the transcript watches its size.
globals.matchMedia = () => ({ matches: false });
globals.ResizeObserver = class {
  observe() {}
  disconnect() {}
};
after(() => {
  Object.assign(globals, saved);
  dom.restore();
});
const { createElement, act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { Workspace } = await import("./Workspace.tsx");

const plan: Plan = {
  goal: "Fix the build",
  revision: 1,
  done: 0,
  tasks: [{ id: "1", depth: 0, title: "Find the failing step", status: "active", group: false, note: "" }],
};

const workspace = (current: Plan | null): WorkspaceState => ({
  generation: 1,
  revision: "gateway:1",
  workspace: "/work",
  session: "session-1",
  title: null,
  model: "default",
  models: [],
  plan: current,
  sessions: [],
  messages: [],
  warnings: [],
  notices: [],
  turn: null,
});

const client = { send: async () => true } as unknown as Client;

function find(predicate: (element: FakeElement) => boolean, name: string) {
  const found = elements(dom.container).find(predicate);
  assert(found, `no ${name}`);
  return found;
}

test("a plan that appears and goes away in the same conversation keeps the conversation and its prompt draft", async () => {
  const errors: unknown[][] = [];
  const consoleError = console.error;
  console.error = (...args: unknown[]) => errors.push(args);
  const root = createRoot(dom.container as unknown as HTMLElement);
  try {
    const render = (current: Plan | null) => act(() => root.render(createElement(Workspace, {
      client, state: workspace(current), error: null, disabled: false, disconnect: () => {},
    })));
    await render(null);
    const prompt = find((element) => element.tagName === "TEXTAREA", "prompt");
    const sendButton = () => find((element) => element.getAttribute("aria-label") === "Send prompt", "Send button");
    assert(sendButton().hasAttribute("disabled"));

    const typed = prompt as FakeElement & { value: string };
    typed.value = "Why does the build fail?";
    await act(() => prompt.dispatchEvent(new FakeEvent("input", { bubbles: true })));
    assert(!sendButton().hasAttribute("disabled"), "the typed prompt did not reach the conversation");

    for (const current of [plan, null, plan]) {
      await render(current);
      const shown = elements(dom.container).some((element) => element.className === "plan");
      assert.equal(shown, current !== null, current ? "the plan did not appear" : "the plan stayed after it went away");
      const now = find((element) => element.tagName === "TEXTAREA", "prompt");
      assert.equal(now, prompt, "the conversation was mounted again");
      assert.equal(typed.value, "Why does the build fail?");
      assert(!sendButton().hasAttribute("disabled"), "the prompt draft was lost");
    }
    assert.deepEqual(errors.map(([message]) => String(message)).filter((message) => /same key/.test(message)), []);
  } finally {
    act(() => root.unmount());
    console.error = consoleError;
  }
});
