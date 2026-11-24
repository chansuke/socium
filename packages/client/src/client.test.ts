import { strict as assert } from "node:assert";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { createClient } from "./client.ts";
import type { ClientOptions } from "./client.ts";
import type { WorkspaceState } from "./protocol.ts";

function workspace(revision: string, changes: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
    generation: 1,
    revision,
    workspace: "workspace",
    session: "session-1",
    title: null,
    model: "local",
    models: [],
    plan: null,
    sessions: [],
    messages: [],
    warnings: [],
    notices: [],
    turn: null,
    ...changes,
  };
}

const json = (status: number, data: unknown) =>
  new Response(JSON.stringify(data), { status, headers: { "Content-Type": "application/json" } });

/** An open event stream the test writes to; aborting the request errors it, as fetch does. */
function eventStream(signal: AbortSignal | null | undefined) {
  const encoder = new TextEncoder();
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  let open = true;
  const body = new ReadableStream<Uint8Array>({
    start(started) {
      controller = started;
    },
    cancel() {
      open = false;
    },
  });
  signal?.addEventListener("abort", () => {
    if (open) controller.error(new DOMException("The operation was aborted.", "AbortError"));
    open = false;
  });
  return {
    response: new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } }),
    write(text: string) {
      if (open) controller.enqueue(encoder.encode(text));
    },
    send(state: WorkspaceState) {
      // Split across chunks, as a network may.
      const event = `id: ${state.revision}\ndata: ${JSON.stringify(state)}\n\n`;
      const middle = Math.floor(event.length / 2);
      this.write(event.slice(0, middle));
      this.write(event.slice(middle));
    },
    end() {
      if (open) controller.close();
      open = false;
    },
    get aborted() {
      return Boolean(signal?.aborted);
    },
  };
}

type Stream = ReturnType<typeof eventStream>;

interface Call {
  url: string;
  init: RequestInit;
}

/**
 * A fetch whose replies the test chooses per path: `reply` returns a
 * Response, or a stream for `/api/events`.
 */
function fakeHost(reply: (call: Call, path: string) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const streams: Stream[] = [];
  const fetchRequest = async (input: string | URL | Request, init: RequestInit = {}) => {
    const call = { url: String(input), init };
    calls.push(call);
    return reply(call, new URL(call.url).pathname);
  };
  return {
    calls,
    streams,
    /** Replies to `/api/events` with a new open stream. */
    stream(call: Call) {
      const created = eventStream(call.init.signal);
      streams.push(created);
      return created.response;
    },
    fetchRequest: fetchRequest as typeof fetch,
    paths: () => calls.map(({ url }) => new URL(url).pathname + new URL(url).search),
  };
}

const fast: ClientOptions = { retryDelay: 5, pollDelay: 5, idleTimeout: 1000 };

function start(context: TestContext, host: ReturnType<typeof fakeHost>, options: ClientOptions = fast) {
  const client = createClient("https://socium.test", "secret-token", host.fetchRequest, options);
  let updates = 0;
  const unsubscribe = client.subscribe(() => updates++);
  context.after(unsubscribe);
  return { client, unsubscribe, updates: () => updates };
}

async function until(condition: () => boolean) {
  for (let tries = 0; !condition(); tries++) {
    assert(tries < 300, "timed out");
    await delay(5);
  }
}

test("the event stream delivers each state without polling", async (context) => {
  const host = fakeHost((call, path) => {
    assert.equal(path, "/api/events");
    return host.stream(call);
  });
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  const [call] = host.calls;
  assert(call);
  assert.equal(call.url, "https://socium.test/api/events?after=");
  assert.deepEqual(call.init.headers, { Accept: "text/event-stream", Authorization: "Bearer secret-token" });

  const [stream] = host.streams;
  assert(stream);
  stream.send(workspace("w:1"));
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
  assert.equal(client.getSnapshot().transport, "events");
  stream.write(": keepalive\n\n");
  stream.send(workspace("w:2", { title: "Renamed" }));
  await until(() => client.getSnapshot().workspace?.revision === "w:2");
  assert.equal(client.getSnapshot().workspace?.title, "Renamed");
  assert.deepEqual(host.paths(), ["/api/events?after="]);
});

test("a state older than a command's reply does not replace it", async (context) => {
  const host = fakeHost((call, path) => {
    if (path === "/api/events") return host.stream(call);
    assert.equal(path, "/api/command");
    return json(200, workspace("w:5", { title: "After the command" }));
  });
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  const [stream] = host.streams;
  assert(stream);
  stream.send(workspace("w:3"));
  await until(() => client.getSnapshot().workspace?.revision === "w:3");

  assert(await client.send({ action: "new" }));
  assert.equal(client.getSnapshot().workspace?.revision, "w:5");
  // Sent by the gateway before the command took effect, received after the reply.
  stream.send(workspace("w:4"));
  await delay(30);
  assert.equal(client.getSnapshot().workspace?.revision, "w:5");
  stream.send(workspace("w:6"));
  await until(() => client.getSnapshot().workspace?.revision === "w:6");
  assert(!stream.aborted);
  assert.equal(host.streams.length, 1);
  // A restarted gateway has a later generation and a new instance, whose counter starts again.
  stream.end();
  await until(() => host.streams.length === 2);
  host.streams[1]?.send(workspace("restarted:1", { generation: 2 }));
  await until(() => client.getSnapshot().workspace?.revision === "restarted:1");
});

test("across a gateway restart, a late command reply from the old gateway does not replace the new state", async (context) => {
  let reply!: (response: Response) => void;
  const host = fakeHost((call, path) => {
    if (path === "/api/events") return host.stream(call);
    return new Promise<Response>((resolve) => reply = resolve);
  });
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("old:3"));
  await until(() => client.getSnapshot().workspace?.revision === "old:3");

  const sent = client.send({ action: "new" });
  await until(() => reply !== undefined);
  host.streams[0]?.send(workspace("new:1", { generation: 2, title: "After the restart" }));
  await until(() => client.getSnapshot().workspace?.revision === "new:1");
  reply(json(200, workspace("old:4")));
  assert(await sent);
  assert.equal(client.getSnapshot().workspace?.revision, "new:1");
  assert.equal(client.getSnapshot().workspace?.title, "After the restart");
});

test("across a gateway restart, a late event from the old gateway does not replace a command reply from the new one", async (context) => {
  const host = fakeHost((call, path) =>
    path === "/api/events" ? host.stream(call) : json(200, workspace("new:2", { generation: 2, title: "After the command" })));
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  const [old] = host.streams;
  assert(old);
  old.send(workspace("old:8"));
  await until(() => client.getSnapshot().workspace?.revision === "old:8");

  assert(await client.send({ action: "new" }));
  assert.equal(client.getSnapshot().workspace?.revision, "new:2");
  old.send(workspace("old:9"));
  await delay(30);
  assert.equal(client.getSnapshot().workspace?.revision, "new:2");
  old.send(workspace("new:3", { generation: 2 }));
  await until(() => client.getSnapshot().workspace?.revision === "new:3");
});

test("a late command reply from a gateway the client never saw does not replace a later gateway's state", async (context) => {
  let reply!: (response: Response) => void;
  const host = fakeHost((call, path) => {
    if (path === "/api/events") return host.stream(call);
    return new Promise<Response>((resolve) => reply = resolve);
  });
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("first:3", { generation: 1 }));
  await until(() => client.getSnapshot().workspace?.revision === "first:3");

  // Reaches a second gateway, which stops before replying in time.
  const sent = client.send({ action: "new" });
  await until(() => reply !== undefined);
  host.streams[0]?.end();
  // The stream reconnects to a third gateway.
  await until(() => host.streams.length === 2);
  host.streams[1]?.send(workspace("third:1", { generation: 3, title: "Third gateway" }));
  await until(() => client.getSnapshot().workspace?.revision === "third:1");
  reply(json(200, workspace("second:7", { generation: 2, title: "Second gateway" })));
  assert(await sent);
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");
  assert.equal(client.getSnapshot().workspace?.title, "Third gateway");
  host.streams[1]?.send(workspace("third:2", { generation: 3 }));
  await until(() => client.getSnapshot().workspace?.revision === "third:2");
});

test("a late event from a gateway the client never saw does not replace a later command reply", async (context) => {
  const host = fakeHost((call, path) =>
    path === "/api/events" ? host.stream(call) : json(200, workspace("third:1", { generation: 3, title: "After the command" })));
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("first:3", { generation: 1 }));
  await until(() => client.getSnapshot().workspace?.revision === "first:3");

  // The stream reconnects to a second gateway, which says nothing yet.
  host.streams[0]?.end();
  await until(() => host.streams.length === 2);
  const second = host.streams[1];
  assert(second);
  await until(() => client.getSnapshot().transport === "events" && client.getSnapshot().connectionError === null);
  // The command, sent later, reaches a third gateway.
  assert(await client.send({ action: "new" }));
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");
  second.send(workspace("second:4", { generation: 2, title: "Second gateway" }));
  await delay(30);
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");
  assert.equal(client.getSnapshot().workspace?.title, "After the command");

  second.end();
  await until(() => host.streams.length === 3);
  host.streams[2]?.send(workspace("third:2", { generation: 3 }));
  await until(() => client.getSnapshot().workspace?.revision === "third:2");
});

test("a command sent later that reaches an earlier generation's gateway is dropped and keeps the later gateway", async (context) => {
  let reply!: (response: Response) => void;
  const host = fakeHost((call, path) => {
    if (path === "/api/events") return host.stream(call);
    return new Promise<Response>((resolve) => reply = resolve);
  });
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  // The stream, sent first, already reached generation 3.
  host.streams[0]?.send(workspace("third:1", { generation: 3, title: "Third gateway" }));
  await until(() => client.getSnapshot().workspace?.revision === "third:1");

  // The command, sent after the stream, lands on generation 2 while it is being replaced.
  const sent = client.send({ action: "new" });
  await until(() => reply !== undefined);
  reply(json(200, workspace("second:9", { generation: 2, title: "Second gateway" })));
  assert(await sent);
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");
  assert.equal(client.getSnapshot().workspace?.title, "Third gateway");
  // Generation 3 stays current.
  host.streams[0]?.send(workspace("third:2", { generation: 3 }));
  await until(() => client.getSnapshot().workspace?.revision === "third:2");
});

test("a stream opened later that reaches an earlier generation's gateway is ignored and keeps the later gateway", async (context) => {
  const host = fakeHost((call, path) =>
    path === "/api/events" ? host.stream(call) : json(200, workspace("third:1", { generation: 3, title: "After the command" })));
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("first:3", { generation: 1 }));
  await until(() => client.getSnapshot().workspace?.revision === "first:3");
  // The command, sent first, reaches generation 3.
  assert(await client.send({ action: "new" }));
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");

  // The stream reopens after the command and reaches generation 2 while it is being replaced.
  host.streams[0]?.end();
  await until(() => host.streams.length === 2);
  host.streams[1]?.send(workspace("second:9", { generation: 2, title: "Second gateway" }));
  await delay(30);
  assert.equal(client.getSnapshot().workspace?.revision, "third:1");
  assert.equal(client.getSnapshot().workspace?.title, "After the command");
  // Generation 3 stays current on the next stream.
  host.streams[1]?.end();
  await until(() => host.streams.length === 3);
  host.streams[2]?.send(workspace("third:2", { generation: 3 }));
  await until(() => client.getSnapshot().workspace?.revision === "third:2");
});

test("a platform that cannot read a streamed response polls instead", async (context) => {
  let polls = 0;
  const host = fakeHost((_call, path) => {
    if (path === "/api/events") {
      // React Native's fetch has no response body stream.
      const response = new Response("", { status: 200 });
      Object.defineProperty(response, "body", { value: undefined });
      return response;
    }
    assert.equal(path, "/api/state");
    polls++;
    return polls === 1 ? json(200, workspace("w:1")) : new Response(null, { status: 204 });
  });
  const { client } = start(context, host);
  await until(() => polls >= 3);
  assert.equal(client.getSnapshot().workspace?.revision, "w:1");
  assert.equal(client.getSnapshot().transport, "polling");
  assert.deepEqual(host.paths().slice(0, 3), ["/api/events?after=", "/api/state?after=", "/api/state?after=w%3A1"]);
  assert.equal(host.paths().filter((path) => path.startsWith("/api/events")).length, 1);
});

test("a stream whose headers never arrive, as with React Native's fetch, leads to polling", async (context) => {
  const host = fakeHost((call, path) => {
    if (path === "/api/state") return json(200, workspace("w:1"));
    assert.equal(path, "/api/events");
    // Answers only once the request is aborted.
    return new Promise<Response>((_resolve, reject) => {
      call.init.signal?.addEventListener("abort", () => reject(new DOMException("The operation was aborted.", "AbortError")));
    });
  });
  const { client } = start(context, host, { ...fast, connectTimeout: 20 });
  await until(() => client.getSnapshot().transport === "polling");
  assert.deepEqual(host.paths().slice(0, 3), ["/api/events?after=", "/api/events?after=", "/api/state?after="]);
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
  assert.equal(client.getSnapshot().connectionError, null);
  assert.equal(host.paths().filter((path) => path.startsWith("/api/events")).length, 2);
});

test("a gateway without the event stream is polled", async (context) => {
  const host = fakeHost((_call, path) =>
    path === "/api/events" ? json(404, { error: "Not found." }) : json(200, workspace("w:1")));
  const { client } = start(context, host);
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
  assert.equal(client.getSnapshot().transport, "polling");
  assert.equal(client.getSnapshot().connectionError, null);
});

test("a proxy that buffers the stream leads to polling", async (context) => {
  const host = fakeHost((call, path) => path === "/api/events" ? host.stream(call) : json(200, workspace("w:1")));
  const { client } = start(context, host, { ...fast, idleTimeout: 20 });
  await until(() => client.getSnapshot().transport === "polling");
  assert.equal(host.streams.length, 2);
  assert(host.streams.every((stream) => stream.aborted));
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
});

test("the stream reconnects after an error and resumes from the last state", async (context) => {
  let attempts = 0;
  const host = fakeHost((call, path) => {
    assert.equal(path, "/api/events");
    attempts++;
    if (attempts === 2) throw new TypeError("Network request failed");
    if (attempts === 3) return json(503, { error: "Socium is starting." });
    return host.stream(call);
  });
  // Long enough to see each error before the next attempt.
  const { client } = start(context, host, { ...fast, retryDelay: 60 });
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("w:1"));
  await until(() => client.getSnapshot().workspace?.revision === "w:1");

  host.streams[0]?.end();
  await until(() => attempts === 2);
  await until(() => client.getSnapshot().connectionError === "Cannot reach the Socium host.");
  assert.equal(client.getSnapshot().tokenRejected, false);
  await until(() => attempts === 3);
  await until(() => client.getSnapshot().connectionError === "Socium is starting.");
  await until(() => host.streams.length === 2);
  assert.equal(host.calls.at(-1)?.url, "https://socium.test/api/events?after=w%3A1");
  host.streams[1]?.send(workspace("w:2"));
  await until(() => client.getSnapshot().workspace?.revision === "w:2");
  assert.equal(client.getSnapshot().connectionError, null);
});

test("a rejected token stops retrying", async (context) => {
  for (const events of [true, false]) {
    const host = fakeHost(() => json(401, { error: "Enter the access token shown on the Socium host." }));
    const { client } = start(context, host, { ...fast, events });
    await until(() => client.getSnapshot().connectionError !== null);
    assert.equal(client.getSnapshot().connectionError, "Enter the access token shown on the Socium host.");
    assert.equal(client.getSnapshot().tokenRejected, true);
    await delay(50);
    assert.equal(host.calls.length, 1, `events: ${events}`);
  }
});

test("unsubscribing closes the stream and stops reconnecting", async (context) => {
  const host = fakeHost((call) => host.stream(call));
  const { unsubscribe } = start(context, host);
  await until(() => host.streams.length === 1);
  unsubscribe();
  assert(host.streams[0]?.aborted);
  await delay(50);
  assert.equal(host.calls.length, 1);
});

test("the host lists and revokes paired devices", async () => {
  const device = { id: "device-1", createdAt: "2026-11-20T00:00:00.000Z", lastUsedAt: null };
  const host = fakeHost((call, path) => {
    if (call.init.method === "DELETE") {
      return path === "/api/devices/device-1" ? new Response(null, { status: 204 }) : json(403, { error: "Use the host access token to manage paired devices." });
    }
    return json(200, { devices: [device] });
  });
  const client = createClient("https://socium.test", "secret-token", host.fetchRequest);
  assert.deepEqual(await client.listDevices(), [device]);
  await client.revokeDevice("device-1");
  await assert.rejects(client.revokeDevice("other/device"), /host access token/);
  assert.equal(host.calls.at(-1)?.url, "https://socium.test/api/devices/other%2Fdevice");
});

test("pairing and device requests stop when their signal aborts", async () => {
  const signals: AbortSignal[] = [];
  const host = fakeHost((call) => new Promise<Response>((_resolve, reject) => {
    const { signal } = call.init;
    assert(signal);
    signals.push(signal);
    signal.addEventListener("abort", () => reject(signal.reason));
  }));
  const client = createClient("https://socium.test", "secret-token", host.fetchRequest);
  for (const start of [
    (signal: AbortSignal) => client.mobilePairingUrl(signal),
    (signal: AbortSignal) => client.listDevices(signal),
    (signal: AbortSignal) => client.revokeDevice("device-1", signal),
  ]) {
    const controller = new AbortController();
    const pending = start(controller.signal);
    await until(() => signals.length === host.calls.length && signals.length > 0);
    controller.abort(new DOMException("The dialog closed.", "AbortError"));
    await assert.rejects(pending, { name: "AbortError" });
    assert(signals.at(-1)?.aborted);
  }
  // Already aborted: nothing is sent.
  const sent = host.calls.length;
  await assert.rejects(client.listDevices(AbortSignal.abort()), { name: "AbortError" });
  assert.equal(host.calls.length, sent);
});

test("a rejected token is final: a command reply that arrives later neither clears it nor restarts polling", async (context) => {
  for (const events of [true, false]) {
    let reply!: (response: Response) => void;
    let rejectNow = false;
    const host = fakeHost((call, path) => {
      if (path === "/api/command") return new Promise<Response>((resolve) => reply = resolve);
      if (rejectNow) return json(401, { error: "Enter the access token shown on the Socium host." });
      return path === "/api/events" ? host.stream(call) : json(200, workspace("w:1"));
    });
    const { client } = start(context, host, { ...fast, events });
    await until(() => client.getSnapshot().workspace?.revision === "w:1" || host.streams.length === 1);
    host.streams[0]?.send(workspace("w:1"));
    await until(() => client.getSnapshot().workspace?.revision === "w:1");

    const sent = client.send({ action: "new" });
    await until(() => reply !== undefined);
    // The device is revoked: the next poll, or the reopened stream, is refused.
    rejectNow = true;
    host.streams[0]?.end();
    await until(() => client.getSnapshot().tokenRejected);
    const calls = host.calls.length;
    reply(json(200, workspace("w:2", { title: "Too late" })));
    assert.equal(await sent, false, `events: ${events}`);
    await delay(50);
    const snapshot = client.getSnapshot();
    assert.equal(snapshot.tokenRejected, true);
    assert.equal(snapshot.connectionError, "Enter the access token shown on the Socium host.");
    assert.equal(snapshot.workspace?.revision, "w:1");
    assert.equal(snapshot.sending, false);
    assert.equal(host.calls.length, calls, "the client sent requests after the token was rejected");
    assert.equal(await client.send({ action: "new" }), false);
    assert.equal(host.calls.length, calls);
  }
});

test("a rejected command token stops the open stream too", async (context) => {
  const host = fakeHost((call, path) =>
    path === "/api/events" ? host.stream(call) : json(401, { error: "Enter the access token shown on the Socium host." }));
  const { client } = start(context, host);
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("w:1"));
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
  assert.equal(await client.send({ action: "new" }), false);
  assert.equal(client.getSnapshot().tokenRejected, true);
  assert.equal(client.getSnapshot().commandError, null);
  assert(host.streams[0]?.aborted);
  host.streams[0]?.send(workspace("w:2"));
  await delay(50);
  assert.equal(client.getSnapshot().workspace?.revision, "w:1");
  assert.equal(host.calls.length, 2);
});

test("an event stream that ends says it is reconnecting and holds commands until a stream opens again", async (context) => {
  let open!: () => void;
  let attempts = 0;
  const host = fakeHost((call, path) => {
    if (path === "/api/command") return json(200, workspace("w:2"));
    attempts++;
    if (attempts === 1) return host.stream(call);
    // The next stream opens when the test says.
    return new Promise<Response>((resolve) => open = () => resolve(host.stream(call)));
  });
  const { client } = start(context, host, { ...fast, connectTimeout: 5000 });
  await until(() => host.streams.length === 1);
  host.streams[0]?.send(workspace("w:1"));
  await until(() => client.getSnapshot().workspace?.revision === "w:1");
  assert.equal(client.getSnapshot().reconnecting, false);

  host.streams[0]?.end();
  await until(() => client.getSnapshot().reconnecting);
  assert.equal(client.getSnapshot().connectionError, null);
  assert.equal(await client.send({ action: "new" }), false);
  assert(!host.paths().includes("/api/command"));

  await until(() => open !== undefined);
  open();
  await until(() => !client.getSnapshot().reconnecting);
  assert(await client.send({ action: "new" }));
  assert.equal(client.getSnapshot().workspace?.revision, "w:2");
});
