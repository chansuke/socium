import type { Command, PairedDevice, WorkspaceState } from "./protocol.ts";

class RequestError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

/** Shown once the host rejects the token, such as for a revoked device. */
export const tokenRejectedMessage = "The host rejected the access token.";

/** How updates arrive: a Server-Sent Event stream, or polling where streams are unavailable. */
export type Transport = "events" | "polling";

interface ClientStatus {
  workspace: WorkspaceState | null;
  connectionError: string | null;
  /**
   * The host rejected the token, such as a revoked device or a host started
   * with a new token. Final for this client: it has stopped every request,
   * ignores replies still on their way, and needs a new token.
   */
  tokenRejected: boolean;
  /**
   * The event stream ended and the client is opening it again, or polling
   * instead; states may be missed until then, so commands wait.
   */
  reconnecting: boolean;
  commandError: string | null;
  sending: boolean;
  transport: Transport | null;
}

const initialStatus: ClientStatus = {
  workspace: null,
  connectionError: null,
  tokenRejected: false,
  reconnecting: false,
  commandError: null,
  sending: false,
  transport: null,
};

export interface ClientOptions {
  /** Set false to poll without trying the event stream. */
  events?: boolean;
  /** Reconnect after this long without a byte; the gateway sends a keepalive every 15 seconds. */
  idleTimeout?: number;
  /**
   * Give up on a stream whose response headers take longer than this; the
   * gateway sends them at once. React Native's fetch only answers once the
   * whole response has arrived, which for a stream is never.
   */
  connectTimeout?: number;
  /** Wait before reconnecting after a connection error. */
  retryDelay?: number;
  /** Wait between polls, and before reopening a stream the gateway ended. */
  pollDelay?: number;
}

/**
 * Streams that sent nothing, in a row, before falling back to polling: ones
 * that connected but stayed silent, and ones whose headers never came.
 */
const maxSilentStreams = 2;

/** Splits a `<instance>:<counter>` revision; the instance changes when the worker restarts. */
function splitRevision(revision: string) {
  const at = revision.lastIndexOf(":");
  return { instance: revision.slice(0, at), count: Number(revision.slice(at + 1)) };
}

/**
 * Whether `next` is a later state than `current`. Gateways number their
 * generations in the order they start, so a state from an earlier generation
 * comes from a replaced gateway, such as a late reply or event, whenever the
 * request that brought it was sent. Within one gateway, the revision counter
 * orders states.
 */
function supersedes(next: WorkspaceState, current: WorkspaceState | null) {
  if (!current) return true;
  if (next.generation !== current.generation) return next.generation > current.generation;
  const nextRevision = splitRevision(next.revision);
  const currentRevision = splitRevision(current.revision);
  // A generation belongs to one gateway, so another instance in it cannot be ordered.
  if (nextRevision.instance !== currentRevision.instance) return false;
  if (!Number.isInteger(nextRevision.count) || !Number.isInteger(currentRevision.count)) {
    return next.revision !== current.revision;
  }
  return nextRevision.count > currentRevision.count;
}

/**
 * A reader for the response body, or null where the platform cannot read a
 * streamed response, such as React Native's fetch.
 */
function bodyReader(response: Response) {
  const body = (response as { body?: ReadableStream<Uint8Array> | null }).body;
  if (!body || typeof body.getReader !== "function" || typeof TextDecoder !== "function") return null;
  return body.getReader();
}

/** Calls `onData` with each event's data and `onBytes` for every chunk, keepalives included. */
async function readEvents(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  onBytes: () => void,
  onData: (data: string) => void,
) {
  const decoder = new TextDecoder();
  let buffer = "";
  let data: string[] = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    onBytes();
    buffer += decoder.decode(value, { stream: true });
    let end: number;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).replace(/\r$/, "");
      buffer = buffer.slice(end + 1);
      if (line === "") {
        if (data.length) onData(data.join("\n"));
        data = [];
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).replace(/^ /, ""));
      }
    }
  }
}

export function createClient(base: string, token: string, fetchRequest = fetch, options: ClientOptions = {}) {
  const { idleTimeout = 45_000, connectTimeout = 10_000, retryDelay = 2500, pollDelay = 650 } = options;
  let status = initialStatus;
  let transport: Transport = options.events === false ? "polling" : "events";
  let silentStreams = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let poll: AbortController | undefined;
  let stream: AbortController | undefined;
  /**
   * Aborted once the host rejects the token, with the 401 as its reason, so
   * every request still on its way, headers or body, fails with it at once.
   */
  const terminal = new AbortController();
  const listeners = new Set<() => void>();

  function update(changes: Partial<ClientStatus>) {
    status = { ...status, ...changes };
    for (const listener of listeners) listener();
  }

  /**
   * Shows `workspace` unless a later state, such as a command's reply, is
   * already shown. Leaves `connectionError` and `reconnecting` alone: only
   * the stream or poll that brings every state may say updates resumed.
   */
  function receive(workspace: WorkspaceState) {
    if (!status.tokenRejected && supersedes(workspace, status.workspace)) update({ workspace });
  }

  function connectionError(error: unknown) {
    return error instanceof RequestError ? error.message : "Cannot reach the Socium host.";
  }

  /**
   * Sends a request with the token. Once the host has rejected the token,
   * fails at once without sending anything, and a 401 makes that final as
   * soon as its headers arrive, without waiting for its body. Callers stop
   * the request on a 401 elsewhere: `requestJson` through `terminal`, and the
   * event stream through `disconnect`.
   */
  async function request(path: string, options?: RequestInit) {
    if (status.tokenRejected) throw new RequestError(401, tokenRejectedMessage);
    const response = await fetchRequest(`${base}/api/${path}`, {
      ...options,
      headers: {
        ...(options?.headers as Record<string, string> | undefined),
        Authorization: `Bearer ${token}`,
        ...(options?.body ? { "Content-Type": "application/json" } : {}),
      },
    });
    if (response.status === 401 || status.tokenRejected) {
      rejectToken();
      throw new RequestError(401, tokenRejectedMessage);
    }
    if (!response.ok) {
      const body = await response.json().catch(() => null);
      throw new RequestError(response.status, typeof body?.error === "string" ? body.error : `Request failed (${response.status}).`);
    }
    return response;
  }

  /**
   * `request` and its JSON body, or null for a 204, with a deadline, without
   * `AbortSignal.timeout`, which React Native lacks, that also stops when
   * `signal` aborts or any request gets a 401. All three cover the body as
   * well as the headers, so a body that stalls after its headers fails too,
   * and nothing is left to settle.
   */
  async function requestJson<T>(path: string, options: RequestInit, signal?: AbortSignal, milliseconds = 10_000): Promise<T | null> {
    signal?.throwIfAborted();
    const controller = new AbortController();
    const stopped = new Promise<never>((_resolve, reject) => {
      controller.signal.addEventListener("abort", () => reject(controller.signal.reason), { once: true });
    });
    // Also rejects when the finally block below aborts after a success.
    stopped.catch(() => {});
    const abort = () => controller.abort(signal?.reason);
    signal?.addEventListener("abort", abort);
    const rejected = () => controller.abort(terminal.signal.reason);
    terminal.signal.addEventListener("abort", rejected);
    const timeout = setTimeout(() => controller.abort(new DOMException("The host did not answer in time.", "TimeoutError")), milliseconds);
    try {
      const response = await Promise.race([request(path, { ...options, signal: controller.signal }), stopped]);
      if (response.status === 204) return null;
      return await Promise.race([response.json() as Promise<T>, stopped]);
    } finally {
      clearTimeout(timeout);
      signal?.removeEventListener("abort", abort);
      terminal.signal.removeEventListener("abort", rejected);
      // Lets go of a body that is no longer read.
      controller.abort();
    }
  }

  /** Stops everything for good: no request with this token can succeed again. */
  function rejectToken() {
    if (status.tokenRejected) return;
    disconnect();
    terminal.abort(new RequestError(401, tokenRejectedMessage));
    update({ tokenRejected: true, connectionError: tokenRejectedMessage, reconnecting: false, sending: false });
  }

  function connect() {
    if (status.tokenRejected) return;
    if (transport === "events") void listen();
    else void refresh();
  }

  function disconnect() {
    clearTimeout(timer);
    timer = undefined;
    const active = [poll, stream];
    poll = undefined;
    stream = undefined;
    for (const controller of active) controller?.abort();
  }

  function fallBackToPolling() {
    transport = "polling";
    update({ transport });
    void refresh();
  }

  async function listen() {
    if (stream || poll || listeners.size === 0 || status.tokenRejected) return;
    const controller = new AbortController();
    stream = controller;
    let idle: ReturnType<typeof setTimeout> | undefined;
    let timedOut = false;
    const abortAfter = (milliseconds: number) => {
      clearTimeout(idle);
      idle = setTimeout(() => {
        timedOut = true;
        controller.abort();
      }, milliseconds);
    };
    const keepAlive = () => abortAfter(idleTimeout);
    abortAfter(connectTimeout);
    let next: "reconnect" | "poll" | "stop" = "reconnect";
    let headers = false;
    let connected = false;
    let received = false;
    try {
      const revision = status.workspace?.revision ?? "";
      const response = await request(`events?after=${encodeURIComponent(revision)}`, {
        headers: { Accept: "text/event-stream" },
        signal: controller.signal,
      });
      headers = true;
      keepAlive();
      const reader = bodyReader(response);
      if (!reader) {
        next = "poll";
        return;
      }
      connected = true;
      if (stream === controller) update({ transport: "events", connectionError: null, reconnecting: false });
      await readEvents(reader, () => {
        received = true;
        keepAlive();
      }, (data) => {
        if (stream === controller) receive(JSON.parse(data));
      });
    } catch (error) {
      if (stream !== controller) return;
      if (timedOut && !headers) {
        // Counted below as a stream that sent nothing; polling shows whether the host is reachable.
      } else if (error instanceof RequestError && error.status === 404) {
        // A gateway without the event stream.
        next = "poll";
      } else {
        if (error instanceof RequestError && error.status === 401) next = "stop";
        else update({ connectionError: connectionError(error) });
      }
    } finally {
      clearTimeout(idle);
      if (stream === controller) {
        stream = undefined;
        controller.abort();
        // A proxy that buffers the stream lets nothing through.
        silentStreams = received ? 0 : silentStreams + Number(connected || (timedOut && !headers));
        if (next === "reconnect" && silentStreams >= maxSilentStreams) next = "poll";
        if (listeners.size && next !== "stop" && !status.tokenRejected) {
          // Until a stream or poll succeeds again, states may be missing.
          update({ reconnecting: true });
          if (next === "poll") fallBackToPolling();
          else timer = setTimeout(listen, status.connectionError ? retryDelay : pollDelay);
        }
      }
    }
  }

  async function refresh() {
    if (poll || stream || listeners.size === 0 || status.tokenRejected) return;
    const controller = new AbortController();
    poll = controller;
    const revision = status.workspace?.revision ?? "";
    let retry = true;
    try {
      const workspace = await requestJson<WorkspaceState>(`state?after=${encodeURIComponent(revision)}`, {}, controller.signal);
      if (poll === controller) {
        if (workspace) receive(workspace);
        if (status.transport !== "polling" || status.connectionError || status.reconnecting) {
          update({ transport: "polling", connectionError: null, reconnecting: false });
        }
      }
    } catch (error) {
      if (poll !== controller) return;
      retry = !(error instanceof RequestError && error.status === 401);
      if (retry) update({ connectionError: connectionError(error) });
    } finally {
      if (poll === controller) {
        poll = undefined;
        if (retry && listeners.size && !status.tokenRejected) {
          timer = setTimeout(refresh, status.connectionError ? retryDelay : pollDelay);
        }
      }
    }
  }

  return {
    getSnapshot: () => status,
    subscribe(listener: () => void) {
      listeners.add(listener);
      if (listeners.size === 1) connect();
      return () => {
        listeners.delete(listener);
        if (!listeners.size) disconnect();
      };
    },
    /** A one-time pairing link for a phone. Needs the host access token; stops when `signal` aborts. */
    async mobilePairingUrl(signal?: AbortSignal) {
      const data = await requestJson<{ url?: unknown }>("pairing", { method: "POST" }, signal);
      if (typeof data?.url !== "string") throw new Error("The host did not send a pairing link.");
      return data.url;
    },
    /** Devices paired with the host, oldest first. Needs the host access token; stops when `signal` aborts. */
    async listDevices(signal?: AbortSignal): Promise<PairedDevice[]> {
      const data = await requestJson<{ devices?: unknown }>("devices", {}, signal);
      if (!Array.isArray(data?.devices)) throw new Error("The host did not send its paired devices.");
      return data.devices;
    },
    /**
     * Revokes a paired device; its token stops working at once. Needs the
     * host access token; stops when `signal` aborts, though the host may
     * already have revoked the device.
     */
    async revokeDevice(id: string, signal?: AbortSignal) {
      await requestJson(`devices/${encodeURIComponent(id)}`, { method: "DELETE" }, signal);
    },
    async send(command: Command) {
      const workspace = status.workspace;
      if (!workspace || status.sending || status.connectionError || status.reconnecting || status.tokenRejected) return false;
      update({ sending: true, commandError: null });
      try {
        const next = await requestJson<WorkspaceState>("command", {
          method: "POST",
          body: JSON.stringify({ ...command, session: workspace.session }),
        }, undefined, 12_000);
        // The token was rejected while this command was on its way.
        if (status.tokenRejected) return false;
        if (!next) throw new Error("The host did not confirm the change.");
        if (transport === "polling") {
          // A GET begun before this mutation may carry a superseded workspace.
          disconnect();
          receive(next);
          void refresh();
        } else {
          // The stream stays open; `receive` drops states older than this one.
          receive(next);
        }
        return true;
      } catch (error) {
        // A rejected token shows as the connection error instead.
        if (!status.tokenRejected) {
          update({ commandError: error instanceof RequestError ? error.message : "The host did not confirm the change." });
        }
        return false;
      } finally {
        update({ sending: false });
      }
    },
  };
}

export type Client = ReturnType<typeof createClient>;

export function mobileEndpoint(value: string): string {
  const url = new URL(value.trim());
  if (url.protocol !== "https:" || url.username || url.password || url.hash || url.search || url.pathname !== "/") {
    throw new Error("Enter an HTTPS host URL, such as https://socium.example.com.");
  }
  return url.origin;
}

export function readPairingUrl(value: string) {
  const error = "Scan a Socium pairing QR code.";
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch {
    throw new Error(error);
  }
  const code = url.hash.slice(1);
  if (url.protocol !== "https:" || url.username || url.password ||
      url.pathname !== "/pair" || url.search || !/^[\w-]{43}$/.test(code)) {
    throw new Error(error);
  }
  return { origin: url.origin, code };
}

export async function redeemPairing(pairing: ReturnType<typeof readPairingUrl>, signal: AbortSignal) {
  let response: Response;
  try {
    response = await fetch(`${pairing.origin}/api/pair`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: pairing.code }),
      redirect: "error",
      signal,
    });
  } catch {
    throw new Error("Cannot reach the Socium host.");
  }
  if (response.status === 401) {
    throw new Error("This QR code is invalid, expired, or already used. Generate a new code on the host.");
  }
  const data = response.ok ? await response.json().catch(() => null) : null;
  if (typeof data?.token !== "string" || !/^[\w-]{43}$/.test(data.token)) {
    throw new Error("The host did not confirm pairing.");
  }
  return { origin: pairing.origin, token: data.token as string };
}
