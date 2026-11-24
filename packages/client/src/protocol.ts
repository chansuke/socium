export type MessageRole = "user" | "assistant" | "system" | "tool" | "tool_error";

export interface Message {
  role: MessageRole;
  text: string;
}

export interface Session {
  id: string;
  title: string;
  readable: boolean;
}

export interface ModelOption {
  alias: string;
  provider: string;
  description: string | null;
  /** Why the host cannot switch to this model now, such as a missing key. */
  unavailable: string | null;
}

/** How far an allow reaches: this call, or what the request offers until Socium exits. */
export type ApprovalScope = "once" | "session";

export interface Approval {
  request: number;
  tool: string;
  summary: string;
  input: string;
  requestedBy: string | null;
  /** What "allow for this session" would allow, such as "`cargo test` commands"; null when not offered. */
  sessionOption: string | null;
}

export type PlanTaskStatus = "pending" | "active" | "blocked" | "done" | "dropped";

export interface PlanTask {
  id: string;
  /** 0 for a top-level task. */
  depth: number;
  title: string;
  status: PlanTaskStatus;
  /** A group's status comes from its children. */
  group: boolean;
  note: string;
}

export interface Plan {
  goal: string;
  revision: number;
  /** Completion of the whole plan, in permille. */
  done: number;
  /** Tree order: each task follows its parent. */
  tasks: PlanTask[];
}

export type ActivityPhase = "requesting" | "thinking" | "responding" | "tool" | "command" | "approval" | "compacting";

/** What the agent is doing now, such as running a command. */
export interface Activity {
  phase: ActivityPhase;
  /** The provider, tool, or command; null when the phase says it all. */
  detail: string | null;
}

export interface Turn {
  id: string;
  messages: Message[];
  draft: string;
  approval: Approval | null;
  activity: Activity | null;
}

export interface WorkspaceState {
  /**
   * Orders gateways: each gateway start takes a larger number than every
   * earlier start, so a state from a smaller generation is from a gateway
   * that a later one has replaced.
   */
  generation: number;
  /** `<instance>:<counter>`; the counter orders states within one gateway. */
  revision: string;
  mobilePairing?: boolean;
  workspace: string;
  session: string;
  title: string | null;
  model: string;
  models: ModelOption[];
  plan: Plan | null;
  sessions: Session[];
  messages: Message[];
  warnings: string[];
  notices: string[];
  turn: Turn | null;
}

/** A phone paired with the host; its token is never sent back. */
export interface PairedDevice {
  id: string;
  /** ISO 8601. */
  createdAt: string;
  /** ISO 8601, to the minute; null until the device first connects. */
  lastUsedAt: string | null;
}

export type Command =
  | { action: "new" }
  | { action: "resume"; target: string }
  | { action: "prompt"; text: string }
  | { action: "cancel"; turn: string }
  | { action: "approve"; turn: string; request: number; allow: boolean; scope?: ApprovalScope }
  | { action: "set_model"; model: string };

export const maxPromptLength = 16_000;
