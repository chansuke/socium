import type { Command, PairedDevice, WorkspaceState } from "./protocol.ts";

class RequestError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

/** How updates arrive: a Server-Sent Event stream, or polling where streams are unavailable. */
export type Transport = "events" | "polling";

interface ClientStatus {
  workspace: WorkspaceState | null;
  connectionError: string | null;
  /**
   * The host rejected the token, such as a revoked device or a host started
   * with a new token; the client has stopped retrying and needs a new token.
   */
  tokenRejected: boolean;
  commandError: string | null;
  sending: boolean;
  transport: Transport | null;
}

const initialStatus: ClientStatus = {
  workspace: null,
  connectionError: null,
  tokenRejected: false,
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
 * Whether `next` is a later state than `current`. A different instance is a
 * restarted worker, whose state replaces the old one, unless `retired` says
 * that instance was itself replaced: then `next` is a late reply or event
 * from a gateway that is gone.
 */
function supersedes(next: WorkspaceState, current: WorkspaceState | null, retired: ReadonlySet<string>) {
  const nextRevision = splitRevision(next.revision);
  if (retired.has(nextRevision.instance)) return false;
  if (!current) return true;
  const currentRevision = splitRevision(current.revision);
  if (nextRevision.instance !== currentRevision.instance ||
      !Number.isInteger(nextRevision.count) || !Number.isInteger(currentRevision.count)) {
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
  const listeners = new Set<() => void>();
  /** Worker instances whose state a later instance has replaced. */
  const retired = new Set<string>();

  function update(changes: Partial<ClientStatus>) {
    status = { ...status, ...changes };
    for (const listener of listeners) listener();
  }

  /** Shows `workspace` unless a later state, such as a command's reply, is already shown. */
  function receive(workspace: WorkspaceState) {
    const current = status.workspace;
    if (supersedes(workspace, current, retired)) {
      const previous = current && splitRevision(current.revision).instance;
      if (previous !== null && previous !== splitRevision(workspace.revision).instance) retired.add(previous);
      update({ workspace, connectionError: null, tokenRejected: false });
    } else if (status.connectionError) {
      update({ connectionError: null, tokenRejected: false });
    }
  }

  function connectionError(error: unknown) {
    return error instanceof RequestError ? error.message : "Cannot reach the Socium host.";
  }

  async function request(path: string, options?: RequestInit) {
    const response = await fetchRequest(`${base}/api/${path}`, {
      ...options,
      headers: {
        ...(options?.headers as Record<string, string> | undefined),
        Authorization: `Bearer ${token}`,
        ...(options?.body ? { "Content-Type": "application/json" } : {}),
      },
    });
    if (!response.ok) {
      const body = await response.json().catch(() => null);
      const message = typeof body?.error === "string" ? body.error : `Request failed (${response.status}).`;
      throw new RequestError(response.status, message);
    }
    return response;
  }

  /** `request` with a deadline, without `AbortSignal.timeout`, which React Native lacks. */
  async function requestWithin(path: string, options: RequestInit, milliseconds = 10_000) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), milliseconds);
    try {
      return await request(path, { ...options, signal: controller.signal });
    } finally {
      clearTimeout(timeout);
    }
  }

  function connect() {
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
    if (stream || poll || listeners.size === 0) return;
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
      if (stream === controller) update({ transport: "events", connectionError: null, tokenRejected: false });
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
        const rejected = error instanceof RequestError && error.status === 401;
        if (rejected) next = "stop";
        update({ connectionError: connectionError(error), tokenRejected: rejected });
      }
    } finally {
      clearTimeout(idle);
      if (stream === controller) {
        stream = undefined;
        controller.abort();
        // A proxy that buffers the stream lets nothing through.
        silentStreams = received ? 0 : silentStreams + Number(connected || (timedOut && !headers));
        if (next === "reconnect" && silentStreams >= maxSilentStreams) next = "poll";
        if (listeners.size && next === "poll") fallBackToPolling();
        else if (listeners.size && next === "reconnect") {
          timer = setTimeout(listen, status.connectionError ? retryDelay : pollDelay);
        }
      }
    }
  }

  async function refresh() {
    if (poll || stream || listeners.size === 0) return;
    const controller = new AbortController();
    poll = controller;
    const timeout = setTimeout(() => controller.abort(), 10_000);
    const revision = status.workspace?.revision ?? "";
    let retry = true;
    try {
      const response = await request(`state?after=${encodeURIComponent(revision)}`, { signal: controller.signal });
      if (response.status !== 204) {
        const workspace: WorkspaceState = await response.json();
        if (poll === controller) receive(workspace);
      } else if (poll === controller && status.connectionError) {
        update({ connectionError: null, tokenRejected: false });
      }
      if (poll === controller && status.transport !== "polling") update({ transport: "polling" });
    } catch (error) {
      if (poll !== controller) return;
      retry = !(error instanceof RequestError && error.status === 401);
      update({ connectionError: connectionError(error), tokenRejected: !retry });
    } finally {
      clearTimeout(timeout);
      if (poll === controller) {
        poll = undefined;
        if (retry && listeners.size) {
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
    async mobilePairingUrl() {
      const response = await requestWithin("pairing", { method: "POST" });
      const { url } = await response.json();
      if (typeof url !== "string") throw new Error("The host did not send a pairing link.");
      return url;
    },
    /** Devices paired with the host, oldest first. Needs the host access token. */
    async listDevices(): Promise<PairedDevice[]> {
      const response = await requestWithin("devices", {});
      const { devices } = await response.json();
      if (!Array.isArray(devices)) throw new Error("The host did not send its paired devices.");
      return devices;
    },
    /** Revokes a paired device; its token stops working at once. Needs the host access token. */
    async revokeDevice(id: string) {
      await requestWithin(`devices/${encodeURIComponent(id)}`, { method: "DELETE" });
    },
    async send(command: Command) {
      const workspace = status.workspace;
      if (!workspace || status.sending || status.connectionError) return false;
      update({ sending: true, commandError: null });
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 12_000);
      try {
        const response = await request("command", {
          method: "POST",
          body: JSON.stringify({ ...command, session: workspace.session }),
          signal: controller.signal,
        });
        const next: WorkspaceState = await response.json();
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
        update({ commandError: error instanceof RequestError ? error.message : "The host did not confirm the change." });
        return false;
      } finally {
        clearTimeout(timeout);
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
