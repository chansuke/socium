import { strict as assert } from "node:assert";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { createClient } from "./client.ts";
import type { ClientOptions } from "./client.ts";
import type { WorkspaceState } from "./protocol.ts";

function workspace(revision: string, changes: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
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
  // A restarted worker has a new instance, whose counter starts again.
  stream.send(workspace("restarted:1"));
  await until(() => client.getSnapshot().workspace?.revision === "restarted:1");
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
