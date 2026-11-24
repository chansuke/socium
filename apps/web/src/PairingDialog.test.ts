import { strict as assert } from "node:assert";
import { after, test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { createClient } from "@socium/client";
import { elements, FakeDialog, installFakeDom } from "./testing/fakeDom.ts";
import type { FakeElement } from "./testing/fakeDom.ts";

// react-dom checks for a DOM once, when it loads, so the fake one comes first.
const dom = installFakeDom();
after(dom.restore);
const { createElement, Profiler, act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { PairingDialog } = await import("./PairingDialog.tsx");

/** A reply whose 200 headers arrive at once and whose body never finishes. */
const stalledBody = () => new Response(new ReadableStream({}), { status: 200, headers: { "Content-Type": "application/json" } });

/**
 * A real client on a host whose every reply stalls after its headers.
 * `bodiesWaitedOn(count)` settles once `count` replies have their body read
 * with `response.json()`, which is where a request waits from then on.
 */
function stalledHost() {
  const calls: { path: string; signal: AbortSignal }[] = [];
  let reads = 0;
  const waiters: { count: number; resolve: () => void }[] = [];
  const fetchRequest = async (input: string | URL | Request, init: RequestInit = {}) => {
    assert(init.signal, "a request was sent without a signal");
    calls.push({ path: new URL(String(input)).pathname, signal: init.signal });
    const response = stalledBody();
    const json = response.json.bind(response);
    response.json = () => {
      const body = json();
      reads++;
      for (const waiter of waiters) if (reads >= waiter.count) waiter.resolve();
      return body;
    };
    return response;
  };
  const bodiesWaitedOn = (count: number) => new Promise<void>((resolve) => {
    if (reads >= count) resolve();
    else waiters.push({ count, resolve });
  });
  return { calls, bodiesWaitedOn, client: createClient("https://socium.test", "secret-token", fetchRequest as typeof fetch) };
}

function button(name: string): FakeElement {
  const found = elements(dom.container).find((element) => element.tagName === "BUTTON" && element.textContent.includes(name));
  assert(found, `no ${name} button`);
  return found;
}

/**
 * Mounts the real dialog with the client, starts a QR code and the device
 * list, and waits until both have their 200 headers and wait on the body.
 */
async function openDialog() {
  const host = stalledHost();
  let commits = 0;
  let closed = 0;
  const root = createRoot(dom.container as unknown as HTMLElement);
  await act(() => root.render(createElement(Profiler, { id: "pairing", onRender: () => commits++ },
    createElement(PairingDialog, { client: host.client, close: () => closed++ }))));
  const dialog = elements(dom.container).find((element) => element instanceof FakeDialog);
  assert(dialog instanceof FakeDialog && dialog.open, "the dialog did not open as a modal");
  await act(() => button("Generate QR code").click());
  await act(() => host.bodiesWaitedOn(2));
  assert.deepEqual(host.calls.map(({ path }) => path), ["/api/devices", "/api/pairing"]);
  assert(host.calls.every(({ signal }) => !signal.aborted));
  assert.match(dom.container.textContent, /Generating…/);
  return { host, root, dialog, commits: () => commits, closed: () => closed };
}

/**
 * After `end`, nothing in flight is left, no request follows, and nothing
 * renders again; the DOM it leaves is returned with the DOM before it.
 */
async function expectEnded(opened: Awaited<ReturnType<typeof openDialog>>, end: () => void) {
  const { host, commits } = opened;
  const before = commits();
  const text = dom.container.textContent;
  act(end);
  assert(host.calls.every(({ signal }) => signal.aborted), "a request was still in flight after the close");
  await act(() => delay(30));
  assert.deepEqual(host.calls.map(({ path }) => path), ["/api/devices", "/api/pairing"], "a request followed the close");
  assert.equal(commits(), before, "the dialog rendered again after the close");
  return { before: text, after: dom.container.textContent };
}

test("the Close button aborts the requests waiting on their bodies at once and nothing follows", async () => {
  const opened = await openDialog();
  const text = await expectEnded(opened, () => button("Close").click());
  assert.equal(text.after, text.before);
  assert.equal(opened.dialog.open, false);
  assert.equal(opened.closed(), 1);
  act(() => opened.root.unmount());
});

test("Escape aborts the requests waiting on their bodies at once and nothing follows", async () => {
  const opened = await openDialog();
  const text = await expectEnded(opened, () => opened.dialog.pressEscape());
  assert.equal(text.after, text.before);
  assert.equal(opened.dialog.open, false);
  assert.equal(opened.closed(), 1);
  act(() => opened.root.unmount());
});

test("unmounting the dialog aborts the requests waiting on their bodies and nothing follows", async () => {
  const opened = await openDialog();
  const text = await expectEnded(opened, () => opened.root.unmount());
  assert.equal(text.after, "");
  assert.equal(opened.closed(), 0);
  assert.equal(dom.container.childNodes.length, 0);
});
