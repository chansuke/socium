import { strict as assert } from "node:assert";
import { once } from "node:events";
import { tmpdir } from "node:os";
import { test } from "node:test";
import type { WorkspaceState } from "@socium/client/protocol";
import type { WorkerCommand } from "./bridge.ts";
import { createHttpServer } from "./http.ts";

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
  assert.equal((await send({ action: "prompt", text: " hello " })).status, 200);
  assert.deepEqual(commands, [{ action: "prompt", text: "hello" }]);

  state.turn = {
    id: "turn-2",
    messages: [],
    draft: "",
    approval: { request: 1, tool: "example", summary: "", input: "Details", requestedBy: null },
  };
  assert.equal((await send({ action: "new" })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-1", request: 1, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 2, allow: true })).status, 409);
  assert.equal((await send({ action: "approve", turn: "turn-2", request: 1, allow: false })).status, 200);
  assert.deepEqual(commands.at(-1), { action: "approve", request: 1, allow: false });
});
