import { strict as assert } from "node:assert";
import { test } from "node:test";
import type { PairedDevice } from "@socium/client/protocol";
import { createClient } from "@socium/client";
import { createPairingSession, dialogHandlers } from "./pairingSession.ts";
import type { PairingSession } from "./pairingSession.ts";

const device: PairedDevice = { id: "device-1-abcdef", createdAt: "2026-11-20T00:00:00.000Z", lastUsedAt: null };

/**
 * A client whose requests stay in flight until the test settles them; each
 * rejects when its signal aborts, as fetch does, and reports a late success
 * otherwise, as a host that answered just after the dialog closed.
 */
function fakeClient() {
  const calls: { method: string; signal?: AbortSignal; settle: () => void }[] = [];
  function call<T>(method: string, value: T, signal?: AbortSignal) {
    return new Promise<T>((resolve, reject) => {
      calls.push({ method, signal, settle: () => resolve(value) });
      signal?.addEventListener("abort", () => reject(signal.reason));
    });
  }
  return {
    calls,
    methods: () => calls.map(({ method }) => method),
    client: {
      mobilePairingUrl: (signal?: AbortSignal) => call("mobilePairingUrl", "https://socium.test/pair#code", signal),
      listDevices: (signal?: AbortSignal) => call("listDevices", [device], signal),
      revokeDevice: (_id: string, signal?: AbortSignal) => call("revokeDevice", undefined, signal),
    },
  };
}

const settled = () => new Promise((resolve) => setTimeout(resolve, 10));

test("closing while a QR code is being created aborts it and sends nothing more", async () => {
  const host = fakeClient();
  const session = createPairingSession(host.client);
  let updates = 0;
  session.subscribe(() => updates++);
  const issuing = session.issue();
  assert.deepEqual(host.methods(), ["mobilePairingUrl"]);
  assert.equal(session.getSnapshot().loading, true);
  const before = updates;

  session.close();
  assert(host.calls[0]?.signal?.aborted);
  await issuing;
  await settled();
  assert.deepEqual(host.methods(), ["mobilePairingUrl"], "a request followed the close");
  assert.equal(updates, before, "the closed dialog changed");
  // Nothing new starts once closed.
  await session.issue();
  await session.loadDevices();
  assert.deepEqual(host.methods(), ["mobilePairingUrl"]);
});

test("closing while a device is being revoked aborts it and does not list devices again", async () => {
  const host = fakeClient();
  const session = createPairingSession(host.client);
  const revoking = session.revoke(device);
  assert.deepEqual(host.methods(), ["revokeDevice"]);
  assert.equal(session.getSnapshot().revoking, device.id);

  session.close();
  assert(host.calls[0]?.signal?.aborted);
  await revoking;
  await settled();
  assert.deepEqual(host.methods(), ["revokeDevice"], "a request followed the close");
  assert.equal(session.getSnapshot().revoking, device.id);
  assert.equal(session.getSnapshot().devicesError, null);
});

test("a reply that arrives after the close, before the abort reached it, changes nothing", async () => {
  const host = fakeClient();
  // As a host whose answer was already on its way: it ignores the abort.
  host.client.listDevices = () => new Promise((resolve) => host.calls.push({ method: "listDevices", settle: () => resolve([device]) }));
  const session = createPairingSession(host.client);
  const listing = session.loadDevices();
  session.close();
  host.calls[0]?.settle();
  await listing;
  assert.equal(session.getSnapshot().devices, null);
  const signal = session.signal;
  session.drawFailed(signal);
  assert.equal(session.getSnapshot().error, null);
});

test("an open dialog creates a code, then lists devices, and reopening starts fresh", async () => {
  const host = fakeClient();
  const session = createPairingSession(host.client);
  const issuing = session.issue();
  host.calls[0]?.settle();
  await settled();
  assert.equal(session.getSnapshot().url, "https://socium.test/pair#code");
  assert.deepEqual(host.methods(), ["mobilePairingUrl", "listDevices"]);
  host.calls[1]?.settle();
  await issuing;
  assert.deepEqual(session.getSnapshot().devices, [device]);

  // React may detach and attach the dialog again in development.
  session.close();
  session.open();
  assert.equal(session.getSnapshot().url, null);
  const revoking = session.revoke(device);
  host.calls[2]?.settle();
  await settled();
  assert.match(session.getSnapshot().feedback ?? "", /Revoked device device-1/);
  host.calls[3]?.settle();
  await revoking;
  assert.deepEqual(host.methods(), ["mobilePairingUrl", "listDevices", "revokeDevice", "listDevices"]);
});

/** A client whose host sends each reply's headers at once and never finishes its body. */
function stalledHost() {
  const calls: RequestInit[] = [];
  const fetchRequest = async (_input: string | URL | Request, init: RequestInit = {}) => {
    calls.push(init);
    return new Response(new ReadableStream({}), { status: 200, headers: { "Content-Type": "application/json" } });
  };
  return { calls, client: createClient("https://socium.test", "host-token", fetchRequest as typeof fetch) };
}

for (const [name, start] of [
  ["creating a QR code", (session: PairingSession) => session.issue()],
  ["listing devices", (session: PairingSession) => session.loadDevices()],
  ["revoking a device", (session: PairingSession) => session.revoke(device)],
] as const) {
  test(`closing while ${name} has its headers but not its body aborts it and sends nothing more`, async () => {
    const host = stalledHost();
    const session = createPairingSession(host.client);
    let updates = 0;
    session.subscribe(() => updates++);
    const pending = start(session);
    await settled();
    assert.equal(host.calls.length, 1);
    const before = updates;

    session.close();
    assert(host.calls[0]?.signal?.aborted);
    await pending;
    await settled();
    assert.equal(updates, before, "the closed dialog changed");
    assert.equal(host.calls.length, 1, "a request followed the close");
  });
}

test("the dialog's Close button and Escape end the session before its close event", async () => {
  for (const handler of ["onCloseClick", "onCancel"] as const) {
    const host = stalledHost();
    const session = createPairingSession(host.client);
    let updates = 0;
    session.subscribe(() => updates++);
    let closed = 0;
    const handlers = dialogHandlers(session, () => closed++);
    const issuing = session.issue();
    await settled();
    const before = updates;

    handlers[handler]();
    assert(session.signal.aborted, `${handler} left the session open`);
    assert(host.calls[0]?.signal?.aborted);
    assert.equal(closed, 0);
    await issuing;
    await settled();
    assert.equal(updates, before);
    assert.equal(host.calls.length, 1);
    // The dialog's own close event comes later.
    handlers.onClose();
    assert.equal(closed, 1);
  }
});
