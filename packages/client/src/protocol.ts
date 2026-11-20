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

export interface Approval {
  request: number;
  tool: string;
  summary: string;
  input: string;
  requestedBy: string | null;
}

export interface Turn {
  id: string;
  messages: Message[];
  draft: string;
  approval: Approval | null;
}

export interface WorkspaceState {
  revision: string;
  workspace: string;
  session: string;
  title: string | null;
  model: string;
  sessions: Session[];
  messages: Message[];
  warnings: string[];
  notices: string[];
  turn: Turn | null;
}

export type Command =
  | { action: "new" }
  | { action: "resume"; target: string }
  | { action: "prompt"; text: string }
  | { action: "cancel"; turn: string }
  | { action: "approve"; turn: string; request: number; allow: boolean };

export const maxPromptLength = 16_000;
