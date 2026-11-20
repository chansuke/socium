import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { basename } from "node:path";
import { createInterface } from "node:readline";
import type { Approval, Message, WorkspaceState } from "@socium/client/protocol";

type Session = Pick<WorkspaceState, "session" | "title" | "model" | "sessions" | "messages" | "warnings">;

type Event =
  | ({ type: "session" } & Session)
  | ({ type: "approval" } & Approval)
  | { type: "running"; turn: string; text: string }
  | { type: "delta"; text: string }
  | { type: "tool"; message: Message }
  | { type: "approval_resolved"; request: number }
  | { type: "outcome"; notices: string[] }
  | { type: "response"; id: string; error: string | null };

export type WorkerCommand =
  | { action: "new" }
  | { action: "resume"; session: string }
  | { action: "prompt"; text: string }
  | { action: "cancel" }
  | { action: "approve"; request: number; allow: boolean };

export class GatewayError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

export function createBridge(binary: string, workspace: string) {
  const env = { ...process.env };
  delete env.SOCIUM_CLIENT_TOKEN;
  const worker = spawn(binary, ["--client-bridge"], {
    cwd: workspace,
    env,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const lines = createInterface({ input: worker.stdout });
  const instance = randomUUID();
  let revision = 0;
  let state: WorkspaceState | null = null;
  let notices: string[] = [];
  let stopped = false;
  let pending: { id: string; finish(error?: GatewayError): void } | undefined;

  function update(changes: Partial<WorkspaceState>) {
    if (state) state = { ...state, ...changes, revision: `${instance}:${++revision}` };
  }

  function updateTurn(changes: Partial<NonNullable<WorkspaceState["turn"]>>) {
    if (state?.turn) update({ turn: { ...state.turn, ...changes } });
  }

  lines.on("line", (line) => {
    let event: Event;
    try {
      event = JSON.parse(line);
    } catch {
      console.error("Socium bridge sent invalid JSON.");
      close();
      return;
    }
    switch (event.type) {
      case "session": {
        const { session, title, model, sessions, messages, warnings } = event;
        state = {
          revision: `${instance}:${++revision}`,
          workspace: basename(workspace), session, title, model, sessions, messages, warnings, notices, turn: null,
        };
        notices = [];
        break;
      }
      case "running":
        update({
          turn: { id: event.turn, messages: [{ role: "user", text: event.text }], draft: "", approval: null },
          notices: [],
        });
        break;
      case "delta":
        if (state?.turn) updateTurn({ draft: state.turn.draft + event.text });
        break;
      case "tool":
        if (state?.turn) {
          const { draft, messages } = state.turn;
          const assistant: Message[] = draft ? [{ role: "assistant", text: draft }] : [];
          updateTurn({ messages: [...messages, ...assistant, event.message], draft: "" });
        }
        break;
      case "approval": {
        const { request, tool, summary, input, requestedBy } = event;
        updateTurn({ approval: { request, tool, summary, input, requestedBy } });
        break;
      }
      case "approval_resolved":
        if (state?.turn?.approval?.request === event.request) updateTurn({ approval: null });
        break;
      case "outcome":
        notices = event.notices;
        break;
      case "response":
        if (pending?.id === event.id) {
          pending.finish(event.error ? new GatewayError(409, event.error) : undefined);
        }
        break;
    }
  });

  function stop(error: GatewayError) {
    stopped = true;
    state = null;
    pending?.finish(error);
  }

  worker.on("error", (error) => {
    console.error("Socium worker:", error.message);
    stop(new GatewayError(503, "The Socium worker could not start. Check the host console."));
  });
  worker.on("close", () => stop(new GatewayError(503, "The Socium worker stopped. Check the host console.")));
  worker.stdin.on("error", () => close());

  function close() {
    if (stopped) return;
    stop(new GatewayError(503, "The Socium worker stopped. Check the host console."));
    worker.stdin.end();
    const timer = setTimeout(() => worker.kill(), 5000);
    timer.unref();
    worker.once("close", () => clearTimeout(timer));
  }

  function snapshot() {
    if (!state) {
      const message = stopped ? "The Socium worker stopped. Check the host console." : "Socium is starting.";
      throw new GatewayError(503, message);
    }
    return state;
  }

  return {
    snapshot,
    close,
    async send(command: WorkerCommand) {
      if (pending) throw new GatewayError(409, "Another client is updating this workspace.");
      snapshot();
      await new Promise<void>((resolve, reject) => {
        const id = randomUUID();
        const timeout = setTimeout(close, 10_000);
        pending = {
          id,
          finish(error) {
            clearTimeout(timeout);
            pending = undefined;
            if (error) reject(error);
            else resolve();
          },
        };
        worker.stdin.write(JSON.stringify({ id, ...command }) + "\n");
      });
      return snapshot();
    },
  };
}

export type Bridge = ReturnType<typeof createBridge>;
