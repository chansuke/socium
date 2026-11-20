import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import type { Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { readPairingUrl } from "@socium/client";
import type { WorkspaceState } from "@socium/client/protocol";
import type { WorkerCommand } from "./bridge.ts";
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

const staticBridge = (state: WorkspaceState) => ({
  snapshot: () => state,
  close() {},
  async send() { return state; },
});

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
  const server = createHttpServer({
    snapshot: () => state,
    close() {},
    async send(command) {
      commands.push(command);
      return state;
    },
  }, token, tmpdir());
  context.after(() => server.close());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address === "object");
  const base = `http://127.0.0.1:${address.port}/api`;
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
  const api = await listen(context, createHttpServer(staticBridge(state), token, tmpdir(), pairing));
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
    listen(context, createHttpServer(staticBridge(workspaceState()), token, tmpdir(), pairing));
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
  const api = await listen(context, createHttpServer(staticBridge(workspaceState()), token, tmpdir(), pairing));
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
  writeFileSync(storePath, "{\"devices\": [{\"id\": 1}]}");
  assert.throws(() => createPairing("https://socium.test/", { storePath }), /damaged/);
  assert.equal(readFileSync(storePath, "utf8"), "{\"devices\": [{\"id\": 1}]}");
});
