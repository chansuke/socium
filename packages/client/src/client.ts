import type { Command, WorkspaceState } from "./protocol.ts";

class RequestError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

interface ClientStatus {
  workspace: WorkspaceState | null;
  connectionError: string | null;
  commandError: string | null;
  sending: boolean;
}

const initialStatus: ClientStatus = {
  workspace: null,
  connectionError: null,
  commandError: null,
  sending: false,
};

export function createClient(base: string, token: string, fetchRequest = fetch) {
  let status = initialStatus;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let poll: AbortController | undefined;
  const listeners = new Set<() => void>();

  function update(changes: Partial<ClientStatus>) {
    status = { ...status, ...changes };
    for (const listener of listeners) listener();
  }

  async function request(path: string, options?: RequestInit) {
    const response = await fetchRequest(`${base}/api/${path}`, {
      ...options,
      headers: {
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

  async function refresh() {
    if (poll || listeners.size === 0) return;
    const controller = new AbortController();
    poll = controller;
    const timeout = setTimeout(() => controller.abort(), 10_000);
    const revision = status.workspace?.revision ?? "";
    let retry = true;
    try {
      const response = await request(`state?after=${encodeURIComponent(revision)}`, { signal: controller.signal });
      if (response.status !== 204) {
        const workspace: WorkspaceState = await response.json();
        if (poll === controller) update({ workspace, connectionError: null });
      } else if (poll === controller && status.connectionError) {
        update({ connectionError: null });
      }
    } catch (error) {
      if (poll !== controller) return;
      retry = !(error instanceof RequestError && error.status === 401);
      update({ connectionError: error instanceof RequestError ? error.message : "Cannot reach the Socium host." });
    } finally {
      clearTimeout(timeout);
      if (poll === controller) {
        poll = undefined;
        if (retry && listeners.size) {
          timer = setTimeout(refresh, status.connectionError ? 2500 : 650);
        }
      }
    }
  }

  return {
    getSnapshot: () => status,
    subscribe(listener: () => void) {
      listeners.add(listener);
      if (listeners.size === 1) void refresh();
      return () => {
        listeners.delete(listener);
        if (listeners.size) return;
        clearTimeout(timer);
        const active = poll;
        poll = undefined;
        active?.abort();
      };
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
        // A GET begun before this mutation may carry a superseded workspace.
        clearTimeout(timer);
        const active = poll;
        poll = undefined;
        active?.abort();
        update({ workspace: next });
        void refresh();
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
