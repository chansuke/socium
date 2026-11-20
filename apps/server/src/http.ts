import { createHash, timingSafeEqual } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import { maxPromptLength } from "@socium/client/protocol";
import type { WorkspaceState } from "@socium/client/protocol";
import { GatewayError } from "./bridge.ts";
import type { Bridge, WorkerCommand } from "./bridge.ts";
import type { Pairing } from "./pairing.ts";

const assetTypes = new Map([
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
]);
const maxBodyBytes = 128 * 1024;

const digest = (value: string) => createHash("sha256").update(value).digest();

function json(response: ServerResponse, status: number, data: unknown) {
  response.writeHead(status, { "Content-Type": "application/json; charset=utf-8" });
  response.end(JSON.stringify(data));
}

async function readJson(request: IncomingMessage) {
  if (request.headers["content-type"]?.split(";")[0]?.trim().toLowerCase() !== "application/json") {
    throw new GatewayError(415, "Send the request as JSON.");
  }
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of request.iterator({ destroyOnReturn: false })) {
    size += chunk.length;
    if (size > maxBodyBytes) throw new GatewayError(413, "The request is too large.");
    chunks.push(chunk);
  }
  try {
    const data: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (data && typeof data === "object" && !Array.isArray(data)) {
      return data as Record<string, unknown>;
    }
  } catch {
    // Malformed JSON and non-object JSON have the same error.
  }
  throw new GatewayError(400, "Invalid request.");
}

function workerCommand(body: Record<string, unknown>, state: WorkspaceState): WorkerCommand {
  if (body.session !== state.session) throw new GatewayError(409, "This conversation changed. Try again.");
  const { action } = body;
  if (action === "cancel" || action === "approve") {
    if (!state.turn || body.turn !== state.turn.id) {
      throw new GatewayError(409, "This turn has already closed.");
    }
    if (action === "cancel") return { action };
    const { allow, request, scope = "once" } = body;
    if (typeof allow !== "boolean" || typeof request !== "number" ||
        (scope !== "once" && scope !== "session") || (scope === "session" && !allow)) {
      throw new GatewayError(400, "Invalid approval answer.");
    }
    const approval = state.turn.approval;
    if (request !== approval?.request) {
      throw new GatewayError(409, "This approval request has already closed.");
    }
    if (scope === "session" && approval.sessionOption === null) {
      throw new GatewayError(409, "This request cannot be allowed for the session.");
    }
    return { action, request, allow, scope };
  }

  if (state.turn) throw new GatewayError(409, "Finish or stop the current turn first.");
  if (action === "new") return { action };
  if (action === "resume" && typeof body.target === "string") {
    const session = state.sessions.find(({ id, readable }) => id === body.target && readable);
    if (!session) throw new GatewayError(404, "This conversation is unavailable.");
    return { action, session: session.id };
  }
  if (action === "set_model" && typeof body.model === "string") {
    const model = state.models.find(({ alias }) => alias === body.model);
    if (!model) throw new GatewayError(404, "This model is unavailable.");
    return { action, model: model.alias };
  }
  if (action === "prompt" && typeof body.text === "string") {
    const text = body.text.trim();
    if (text.length > 0 && text.length <= maxPromptLength) return { action, text };
    throw new GatewayError(400, `Enter a prompt of 1 to ${maxPromptLength} characters.`);
  }
  throw new GatewayError(400, "Invalid command.");
}

export interface GatewayTimeouts {
  /** Longest time to receive a whole request; event streams are exempt once open. */
  requestTimeout?: number;
  /** How often the server looks for requests over that limit. */
  connectionsCheckingInterval?: number;
  /** How often an idle event stream sends a comment, so proxies keep it open. */
  keepaliveInterval?: number;
}

export function createHttpServer(
  bridge: Bridge,
  token: string,
  webRoot: string,
  pairing?: Pairing,
  { requestTimeout = 15_000, connectionsCheckingInterval = 30_000, keepaliveInterval = 15_000 }: GatewayTimeouts = {},
) {
  const expected = digest(`Bearer ${token}`);
  const clientState = (state: WorkspaceState) => ({ ...state, mobilePairing: Boolean(pairing) });

  /**
   * Sends each new workspace state as a Server-Sent Event. A burst of
   * changes is sent as its latest state, and a slow client gets the latest
   * state once it has drained instead of every state in between.
   */
  function streamEvents(request: IncomingMessage, response: ServerResponse, after: string | null, authorized: () => boolean) {
    let latest: WorkspaceState | null = bridge.snapshot();
    let sent = after;
    let scheduled = false;
    // The request ended with its headers; nothing should time out the reply.
    request.socket.setTimeout(0);
    response.writeHead(200, { "Content-Type": "text/event-stream; charset=utf-8", "X-Accel-Buffering": "no" });
    response.flushHeaders();

    function flush() {
      scheduled = false;
      if (response.writableEnded) return;
      if (!latest || !authorized()) {
        response.end();
        return;
      }
      if (latest.revision === sent || response.writableNeedDrain) return;
      sent = latest.revision;
      response.write(`id: ${latest.revision}\ndata: ${JSON.stringify(clientState(latest))}\n\n`);
    }
    function schedule() {
      if (scheduled) return;
      scheduled = true;
      setImmediate(flush);
    }

    const unsubscribe = bridge.subscribe((state) => {
      latest = state;
      schedule();
    });
    const keepalive = setInterval(() => {
      if (!authorized()) response.end();
      else if (!response.writableNeedDrain) response.write(": keepalive\n\n");
    }, keepaliveInterval);
    response.on("drain", schedule);
    response.on("close", () => {
      clearInterval(keepalive);
      unsubscribe();
    });
    flush();
  }

  return createServer({ headersTimeout: Math.min(5000, requestTimeout), requestTimeout, connectionsCheckingInterval }, async (request, response) => {
    response.setHeader("Cache-Control", "no-store");
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Referrer-Policy", "no-referrer");
    response.setHeader("Content-Security-Policy", "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'");

    try {
      const url = new URL(request.url ?? "/", "http://localhost");
      if (url.pathname.startsWith("/api/")) {
        if (request.method === "POST" && url.pathname === "/api/pair" && pairing) {
          const { code } = await readJson(request);
          const pairedToken = typeof code === "string" && /^[\w-]{43}$/.test(code) ? pairing.redeem(code) : null;
          if (!pairedToken) throw new GatewayError(401, "This pairing code is invalid, expired, or already used.");
          json(response, 200, { token: pairedToken });
          return;
        }
        const authorization = request.headers.authorization ?? "";
        const hostAccess = timingSafeEqual(digest(authorization), expected);
        if (!hostAccess && !pairing?.authorizes(authorization)) {
          throw new GatewayError(401, "Enter the access token shown on the Socium host.");
        }
        if (request.method === "POST" && url.pathname === "/api/pairing" && pairing) {
          if (!hostAccess) throw new GatewayError(403, "Use the host access token to generate a pairing QR code.");
          json(response, 200, { url: pairing.issue() });
          return;
        }
        if (request.method === "GET" && url.pathname === "/api/devices") {
          if (!hostAccess) throw new GatewayError(403, "Use the host access token to manage paired devices.");
          json(response, 200, { devices: pairing?.devices() ?? [] });
          return;
        }
        const device = /^\/api\/devices\/([\w-]{1,64})$/.exec(url.pathname);
        if (request.method === "DELETE" && device) {
          if (!hostAccess) throw new GatewayError(403, "Use the host access token to manage paired devices.");
          if (!pairing?.revoke(device[1])) throw new GatewayError(404, "This device is not paired.");
          response.writeHead(204).end();
          return;
        }
        if (request.method === "GET" && url.pathname === "/api/events") {
          const lastEventId = request.headers["last-event-id"];
          const after = url.searchParams.get("after") ?? (typeof lastEventId === "string" ? lastEventId : null);
          // A revoked device loses its open stream too.
          streamEvents(request, response, after, () => hostAccess || Boolean(pairing?.authorizes(authorization)));
          return;
        }
        if (request.method === "GET" && url.pathname === "/api/state") {
          const state = bridge.snapshot();
          if (url.searchParams.get("after") === state.revision) response.writeHead(204).end();
          else json(response, 200, clientState(state));
          return;
        }
        if (request.method === "POST" && url.pathname === "/api/command") {
          const body = await readJson(request);
          const command = workerCommand(body, bridge.snapshot());
          json(response, 200, clientState(await bridge.send(command)));
          return;
        }
      } else if (request.method === "GET" || request.method === "HEAD") {
        const asset = /^\/assets\/[\w.-]+(\.js|\.css)$/.exec(url.pathname);
        const file = url.pathname === "/" ? "index.html" : asset?.[0].slice(1);
        if (file) {
          let contents: Buffer;
          try {
            contents = await readFile(join(webRoot, file));
          } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
            throw new GatewayError(404, file === "index.html" ? "Build the web client with npm run build:web." : "Asset not found.");
          }
          const contentType = asset ? assetTypes.get(asset[1]) : "text/html; charset=utf-8";
          response.writeHead(200, { "Content-Type": contentType ?? "application/octet-stream" });
          response.end(request.method === "HEAD" ? undefined : contents);
          return;
        }
      }
      throw new GatewayError(404, "Not found.");
    } catch (error) {
      // The client went away mid-request, such as after a request timeout.
      if (response.destroyed && (error as NodeJS.ErrnoException).code === "ECONNRESET") return;
      if (error instanceof GatewayError) {
        if (error.status === 413) response.setHeader("Connection", "close");
        json(response, error.status, { error: error.message });
      } else {
        console.error(error);
        json(response, 500, { error: "The Socium gateway encountered an error." });
      }
    }
  });
}
