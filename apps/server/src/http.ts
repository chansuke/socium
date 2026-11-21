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
    const { allow, request } = body;
    const forSession = body.forSession ?? false;
    if (typeof allow !== "boolean" || typeof request !== "number" || typeof forSession !== "boolean") {
      throw new GatewayError(400, "Invalid approval answer.");
    }
    if (request !== state.turn.approval?.request) {
      throw new GatewayError(409, "This approval request has already closed.");
    }
    if (forSession && allow && state.turn.approval.sessionOption === null) {
      throw new GatewayError(400, "This request can only be allowed once.");
    }
    return { action, request, allow, forSession: forSession && allow };
  }

  if (state.turn) throw new GatewayError(409, "Finish or stop the current turn first.");
  if (action === "new") return { action };
  if (action === "resume" && typeof body.target === "string") {
    const session = state.sessions.find(({ id, readable }) => id === body.target && readable);
    if (!session) throw new GatewayError(404, "This conversation is unavailable.");
    return { action, session: session.id };
  }
  if (action === "prompt" && typeof body.text === "string") {
    const text = body.text.trim();
    if (text.length > 0 && text.length <= maxPromptLength) return { action, text };
    throw new GatewayError(400, `Enter a prompt of 1 to ${maxPromptLength} characters.`);
  }
  throw new GatewayError(400, "Invalid command.");
}

export function createHttpServer(bridge: Bridge, token: string, webRoot: string, pairing?: Pairing) {
  const expected = digest(`Bearer ${token}`);
  const clientState = (state: WorkspaceState) => ({ ...state, mobilePairing: Boolean(pairing) });

  return createServer({ headersTimeout: 5000, requestTimeout: 15_000 }, async (request, response) => {
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
