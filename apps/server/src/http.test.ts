import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { connect } from "node:net";
import { setTimeout as delay } from "node:timers/promises";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import type { Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { readPairingUrl } from "@socium/client";
import type { WorkspaceState } from "@socium/client/protocol";
import { GatewayError } from "./bridge.ts";
import type { StateListener, WorkerCommand } from "./bridge.ts";
import { createHttpServer } from "./http.ts";
import { createPairing } from "./pairing.ts";
import type { Pairing } from "./pairing.ts";

/** A directory removed after the test, for pairing stores. */
function temporaryDirectory(context: TestContext) {
  const directory = mkdtempSync(join(tmpdir(), "socium-gateway-"));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

async function listen(context: TestContext, server: Server) {
  context.after(() => server.close());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address === "object");
  return `http://127.0.0.1:${address.port}/api`;
}

/** A bridge whose state the test sets; `emit` tells subscribers as the real bridge does. */
function fakeBridge(initial: WorkspaceState, send: (command: WorkerCommand) => Promise<void> = async () => {}) {
  let state: WorkspaceState | null = initial;
  const listeners = new Set<StateListener>();
  return {
    listeners,
    snapshot() {
      if (!state) throw new GatewayError(503, "The Socium worker stopped. Check the host console.");
      return state;
    },
    close() {},
    subscribe(listener: StateListener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    async send(command: WorkerCommand) {
      await send(command);
      return this.snapshot();
    },
    emit(next: WorkspaceState | null) {
      state = next;
      for (const listener of listeners) listener(state);
    },
  };
}

/** Reads Server-Sent Event blocks; null once the stream ends. */
function eventReader(response: Response) {
  assert(response.body);
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let buffer = "";
  async function block(): Promise<string | null> {
    for (;;) {
      const end = buffer.indexOf("\n\n");
      if (end >= 0) {
        const next = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        return next;
      }
      const { value, done } = await reader.read();
      if (done) return null;
      buffer += value;
    }
  }
  return {
    block,
    /** The next state event, skipping comments. */
    async state(): Promise<(WorkspaceState & { id: string }) | null> {
      for (;;) {
        const next = await block();
        if (next === null) return null;
        if (next.startsWith(":")) continue;
        const id = /^id: (.*)$/m.exec(next)?.[1];
        const data = /^data: (.*)$/m.exec(next)?.[1];
        assert(id && data, next);
        return { ...JSON.parse(data), id };
      }
    },
  };
}

async function until(condition: () => boolean) {
  for (let tries = 0; !condition(); tries++) {
    assert(tries < 200, "timed out");
    await delay(10);
  }
}

/** Pairs one device through the gateway and returns its token. */
async function pairDevice(api: string, hostToken: string) {
  const issued = await fetch(`${api}/pairing`, { method: "POST", headers: { Authorization: `Bearer ${hostToken}` } });
  const { code } = readPairingUrl((await issued.json()).url);
  const redeemed = await fetch(`${api}/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  assert.equal(redeemed.status, 200);
  return (await redeemed.json()).token as string;
}

function workspaceState(changes: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
    revision: "revision-1",
    workspace: "workspace",
    session: "session-1",
    title: null,
    model: "mock",
    models: [
      { alias: "mock", provider: "local", description: null, unavailable: null },
      { alias: "spare", provider: "local", description: "Second model", unavailable: null },
    ],
    plan: null,
    sessions: [],
    messages: [],
    warnings: [],
    notices: [],
    turn: null,
    ...changes,
  };
}

test("gateway gates commands by access token, conversation, turn, and exact approval", async (context) => {
  const token = "client-gateway-test-token";
  const state = workspaceState();
  const commands: WorkerCommand[] = [];
  const base = await listen(context, createHttpServer(fakeBridge(state, async (command) => {
    commands.push(command);
  }), token, tmpdir()));
  const auth = { Authorization: `Bearer ${token}` };
  const send = (body: object, headers: Record<string, string> = auth) =>
    fetch(`${base}/command`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify({ session: state.session, ...body }),
    });

  assert.equal((await fetch(`${base}/state`)).status, 401);
  assert.equal((await send({ action: "new" }, {})).status, 401);
  assert.equal((await fetch(`${base}/state?after=revision-1`, { headers: auth })).status, 204);
  assert.equal((await send({ action: "prompt", session: "other", text: "hello" })).status, 409);
  assert.equal((await send({ action: "prompt", text: " " })).status, 400);
  assert.equal((await send({ action: "prompt", text: "x".repeat(140_000) })).status, 413);
  assert.equal((await send({ action: "prompt", text: "hello" }, { ...auth, "Content-Type": "text/plain" })).status, 415);
  assert.equal((await send({ action: "prompt", text: " hello " }, { ...auth, "Content-Type": "Application/JSON ; charset=utf-8" })).status, 200);
  assert.deepEqual(commands, [{ action: "prompt", text: "hello" }]);
  assert.equal((await send({ action: "set_model", model: "missing" })).status, 404);
  assert.equal((await send({ action: "set_model", model: 7 })).status, 400);
  assert.equal((await send({ action: "set_model", model: "spare" })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "set_model", model: "spare" });

  state.turn = {
    id: "turn-2",
    messages: [],
    draft: "",
    approval: { request: 1, tool: "example", summary: "", input: "Details", requestedBy: null, sessionOption: null },
    activity: { phase: "approval", detail: "example" },
  };
  assert.equal((await send({ action: "new" })).status, 409);
  assert.equal((await send({ action: "set_model", model: "spare" })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-1", request: 1, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 2, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: true, scope: "session" })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: false })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "approve", request: 1, allow: false, scope: "once" });

  state.turn.approval = { request: 2, tool: "run_command", summary: "cargo test", input: "", requestedBy: null, sessionOption: "`cargo test` commands" };
  const approve = (answer: object) => send({ action: "approve", turn: "turn-2", request: 2, ...answer });
  assert.equal((await approve({ allow: true, scope: "forever" })).status, 400);
  assert.equal((await approve({ allow: false, scope: "session" })).status, 400);
  assert.equal((await approve({ allow: true, scope: "session" })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "approve", request: 2, allow: true, scope: "session" });
});

test("pairing codes are issued by the host and exchanged once for a device token", async (context) => {
  const state = workspaceState();
  let time = 0;
  const token = "host-access-token";
  const pairing = createPairing("https://socium.test/", {
    clock: () => time,
    storePath: join(temporaryDirectory(context), "paired-devices.json"),
  });
  const api = await listen(context, createHttpServer(fakeBridge(state), token, tmpdir(), pairing));
  const host = { Authorization: `Bearer ${token}` };
  const issue = (headers?: Record<string, string>) => fetch(`${api}/pairing`, { method: "POST", headers });
  const redeem = (code: string) => fetch(`${api}/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  const pairingUrl = async () => readPairingUrl((await (await issue(host)).json()).url);

  assert.equal((await issue()).status, 401);
  const expired = await pairingUrl();
  assert.equal(expired.origin, "https://socium.test");
  time += 5 * 60_000;
  assert.equal((await redeem(expired.code)).status, 401);
  const replaced = await pairingUrl();
  const active = await pairingUrl();
  assert.equal((await redeem(replaced.code)).status, 401);

  const requests = await Promise.all([redeem(active.code), redeem(active.code)]);
  assert.deepEqual(requests.map(({ status }) => status).sort(), [200, 401]);
  const result = requests.find(({ ok }) => ok);
  assert(result);
  const deviceToken = (await result.json()).token;
  assert.notEqual(deviceToken, token);
  const device = { Authorization: `Bearer ${deviceToken}` };
  assert.equal((await fetch(`${api}/state`, { headers: device })).status, 200);
  assert.equal((await issue(device)).status, 403);

  for (const invalid of ["http://socium.test/pair#test", `https://user@socium.test/pair#${active.code}`, `https://socium.test/login#${active.code}`]) {
    assert.throws(() => readPairingUrl(invalid));
  }
});

test("paired devices persist as hashes in a private store and survive a restart", async (context) => {
  const token = "host-access-token";
  const storePath = join(temporaryDirectory(context), "state", "socium", "paired-devices.json");
  let time = Date.parse("2026-11-20T00:00:00Z");
  const start = (pairing: Pairing) =>
    listen(context, createHttpServer(fakeBridge(workspaceState()), token, tmpdir(), pairing));
  const restart = () => createPairing("https://socium.test/", { clock: () => time, storePath });

  const first = await start(restart());
  const deviceToken = await pairDevice(first, token);
  const device = { Authorization: `Bearer ${deviceToken}` };
  assert.equal(statSync(join(storePath, "..")).mode & 0o777, 0o700);
  assert.equal(statSync(storePath).mode & 0o777, 0o600);
  const stored = readFileSync(storePath, "utf8");
  assert(!stored.includes(deviceToken));
  assert(stored.includes(createHash("sha256").update(deviceToken).digest("hex")));

  time += 90_000;
  const api = await start(restart());
  assert.equal((await fetch(`${api}/state`, { headers: device })).status, 200);
  const listed = await (await fetch(`${api}/devices`, { headers: { Authorization: `Bearer ${token}` } })).json();
  assert.deepEqual(Object.keys(listed.devices[0]).sort(), ["createdAt", "id", "lastUsedAt"]);
  assert.equal(listed.devices[0].createdAt, "2026-11-20T00:00:00.000Z");
  assert.equal(listed.devices[0].lastUsedAt, "2026-11-20T00:01:30.000Z");
  // Polling within the minute does not rewrite the store.
  const written = statSync(storePath).mtimeMs;
  time += 1000;
  assert.equal((await fetch(`${api}/state`, { headers: device })).status, 200);
  assert.equal(statSync(storePath).mtimeMs, written);
});

test("only the host lists and revokes paired devices, and revocation is immediate and persisted", async (context) => {
  const token = "host-access-token";
  const host = { Authorization: `Bearer ${token}` };
  const storePath = join(temporaryDirectory(context), "paired-devices.json");
  const pairing = createPairing("https://socium.test/", { storePath });
  const api = await listen(context, createHttpServer(fakeBridge(workspaceState()), token, tmpdir(), pairing));
  const kept = { Authorization: `Bearer ${await pairDevice(api, token)}` };
  const revoked = { Authorization: `Bearer ${await pairDevice(api, token)}` };
  const list = async () => (await (await fetch(`${api}/devices`, { headers: host })).json()).devices as { id: string }[];
  const revoke = (id: string, headers: Record<string, string>) => fetch(`${api}/devices/${id}`, { method: "DELETE", headers });

  const [first, second] = await list();
  assert(first && second);
  assert.equal((await fetch(`${api}/devices`, { headers: kept })).status, 403);
  assert.equal((await fetch(`${api}/devices`)).status, 401);
  assert.equal((await revoke(second.id, kept)).status, 403);
  assert.equal((await revoke("not-a-device", host)).status, 404);
  assert.equal((await revoke(second.id, host)).status, 204);
  assert.equal((await fetch(`${api}/state`, { headers: revoked })).status, 401);
  assert.equal((await fetch(`${api}/state`, { headers: kept })).status, 200);
  assert.deepEqual((await list()).map(({ id }) => id), [first.id]);

  const reloaded = createPairing("https://socium.test/", { storePath });
  assert.equal(reloaded.authorizes(revoked.Authorization), false);
  assert.equal(reloaded.authorizes(kept.Authorization), true);
  // A revocation by another gateway on the same store applies here too.
  assert(reloaded.revoke(first.id));
  assert.equal((await fetch(`${api}/state`, { headers: kept })).status, 401);
});

test("a damaged pairing store stops the gateway instead of dropping devices", (context) => {
  const storePath = join(temporaryDirectory(context), "paired-devices.json");
  writeFileSync(storePath, "{\"devices\": [{\"id\": 1}]}", { mode: 0o600 });
  assert.throws(() => createPairing("https://socium.test/", { storePath }), /damaged/);
  assert.equal(readFileSync(storePath, "utf8"), "{\"devices\": [{\"id\": 1}]}");
});

test("the event stream sends the snapshot, each change, and keepalives, and lets go on disconnect", { timeout: 10_000 }, async (context) => {
  const token = "host-access-token";
  const host = { Authorization: `Bearer ${token}` };
  const bridge = fakeBridge(workspaceState());
  const api = await listen(context, createHttpServer(bridge, token, tmpdir(), undefined, { keepaliveInterval: 30 }));
  const open = (query = "", headers: Record<string, string> = {}, signal?: AbortSignal) =>
    fetch(`${api}/events${query}`, { headers: { ...host, ...headers }, signal });

  assert.equal((await fetch(`${api}/events`)).status, 401);
  const controller = new AbortController();
  const response = await open("", {}, controller.signal);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "text/event-stream; charset=utf-8");
  const events = eventReader(response);
  const first = await events.state();
  assert.equal(first?.id, "revision-1");
  assert.equal(first?.mobilePairing, false);

  bridge.emit(workspaceState({ revision: "revision-2", title: "Renamed" }));
  const second = await events.state();
  assert.equal(second?.revision, "revision-2");
  assert.equal(second?.title, "Renamed");
  // A burst of changes arrives as its latest state.
  bridge.emit(workspaceState({ revision: "revision-3" }));
  bridge.emit(workspaceState({ revision: "revision-4" }));
  assert.equal((await events.state())?.revision, "revision-4");
  while (await events.block() !== ": keepalive");

  assert.equal(bridge.listeners.size, 1);
  controller.abort();
  await until(() => bridge.listeners.size === 0);

  // A client that has the current state waits for the next one.
  for (const resume of [() => open("?after=revision-4"), () => open("", { "Last-Event-ID": "revision-4" })]) {
    const reader = eventReader(await resume());
    await until(() => bridge.listeners.size === 1);
    bridge.emit(workspaceState({ revision: "revision-5" }));
    assert.equal((await reader.state())?.revision, "revision-5");
    bridge.emit(null);
    assert.equal(await reader.state(), null);
    await until(() => bridge.listeners.size === 0);
    bridge.emit(workspaceState({ revision: "revision-4" }));
  }
  bridge.emit(null);
  assert.equal((await open()).status, 503);
});

test("event streams outlive the request timeout that still closes slow requests", { timeout: 10_000 }, async (context) => {
  const token = "host-access-token";
  const bridge = fakeBridge(workspaceState());
  const timeouts = { requestTimeout: 200, connectionsCheckingInterval: 50, keepaliveInterval: 60_000 };
  const api = await listen(context, createHttpServer(bridge, token, tmpdir(), undefined, timeouts));
  const controller = new AbortController();
  context.after(() => controller.abort());
  const events = eventReader(await fetch(`${api}/events`, { headers: { Authorization: `Bearer ${token}` }, signal: controller.signal }));
  assert.equal((await events.state())?.revision, "revision-1");

  const { port } = new URL(api);
  const slow = connect(Number(port), "127.0.0.1");
  slow.write(`POST /api/command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer ${token}\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{`);
  let reply = "";
  slow.on("data", (chunk) => reply += chunk);
  await once(slow, "close");
  assert.match(reply, /^HTTP\/1\.1 408/);

  await delay(500);
  bridge.emit(workspaceState({ revision: "revision-2" }));
  assert.equal((await events.state())?.revision, "revision-2");
});

test("a revoked device loses its open event stream", { timeout: 10_000 }, async (context) => {
  const token = "host-access-token";
  const bridge = fakeBridge(workspaceState());
  const pairing = createPairing("https://socium.test/", { storePath: join(temporaryDirectory(context), "paired-devices.json") });
  const api = await listen(context, createHttpServer(bridge, token, tmpdir(), pairing));
  const deviceToken = await pairDevice(api, token);
  const events = eventReader(await fetch(`${api}/events`, { headers: { Authorization: `Bearer ${deviceToken}` } }));
  assert.equal((await events.state())?.mobilePairing, true);

  const [device] = pairing.devices();
  assert(device);
  const revoked = await fetch(`${api}/devices/${device.id}`, { method: "DELETE", headers: { Authorization: `Bearer ${token}` } });
  assert.equal(revoked.status, 204);
  bridge.emit(workspaceState({ revision: "revision-2" }));
  assert.equal(await events.state(), null);
  await until(() => bridge.listeners.size === 0);
});
