import { strict as assert } from "node:assert";
import { once } from "node:events";
import { tmpdir } from "node:os";
import { test } from "node:test";
import { readPairingUrl } from "@socium/client";
import type { WorkspaceState } from "@socium/client/protocol";
import type { WorkerCommand } from "./bridge.ts";
import { createHttpServer } from "./http.ts";
import { createPairing } from "./pairing.ts";

test("gateway gates commands by access token, conversation, turn, and exact approval", async (context) => {
  const token = "client-gateway-test-token";
  const state: WorkspaceState = {
    revision: "revision-1",
    workspace: "workspace",
    session: "session-1",
    title: null,
    model: "mock",
    sessions: [],
    messages: [],
    warnings: [],
    notices: [],
    turn: null,
  };
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

  state.turn = {
    id: "turn-2",
    messages: [],
    draft: "",
    approval: { request: 1, tool: "example", summary: "", input: "Details", requestedBy: null, sessionOption: null },
  };
  assert.equal((await send({ action: "new" })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-1", request: 1, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 2, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: false })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "approve", request: 1, allow: false, forSession: false });
  // Allowing for the session needs a request that offers it.
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: true, forSession: true })).status, 400);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: true, forSession: "yes" })).status, 400);
  state.turn.approval = { request: 1, tool: "mcp__github__create_issue", summary: "", input: "Details", requestedBy: null, sessionOption: "`create_issue` of github" };
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: true, forSession: true })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "approve", request: 1, allow: true, forSession: true });
});

test("pairing codes are issued by the host and exchanged once for a device token", async (context) => {
  const state: WorkspaceState = {
    revision: "revision",
    workspace: "workspace",
    session: "session",
    title: null,
    model: "mock",
    sessions: [],
    messages: [],
    warnings: [],
    notices: [],
    turn: null,
  };
  let time = 0;
  const token = "host-access-token";
  const server = createHttpServer({
    snapshot: () => state,
    close() {},
    async send() { return state; },
  }, token, tmpdir(), createPairing("https://socium.test/", () => time));
  context.after(() => server.close());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address === "object");
  const api = `http://127.0.0.1:${address.port}/api`;
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
