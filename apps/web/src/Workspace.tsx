import { useCallback, useRef, useState } from "react";
import type { CSSProperties, FormEvent } from "react";
import {
  ArrowUp, Circle, CircleAlert, CircleCheck, CircleDot, CircleSlash, Folder, ListChecks, LoaderCircle, Menu,
  MessageSquare, Plus, QrCode, Square, Terminal, Unplug,
} from "lucide-react";
import type { Client } from "@socium/client";
import { maxPromptLength } from "@socium/client/protocol";
import type { Message, Plan, PlanTaskStatus, WorkspaceState } from "@socium/client/protocol";
import { describeActivity, planProgress, planStatusLabels } from "@socium/client/progress";
import { AssistantMarkdown } from "./assistantMarkdown";
import { PairingDialog } from "./PairingDialog";

const starters = [
  ["Explore the workspace", "Explain the structure of this repository and its main entry points."],
  ["Review local changes", "Review the current changes and flag bugs or missing tests."],
  ["Plan a fix", "Suggest a plan to fix an issue. Ask me for the details first."],
];

export function Brand() {
  return <span className="brand"><span className="mark"><Terminal size={19} /></span>socium</span>;
}

/** The connected host: its conversations, the agent's plan, and the current conversation. */
export function Workspace({
  client, state, error, disabled, disconnect,
}: {
  client: Client;
  state: WorkspaceState;
  error: string | null;
  disabled: boolean;
  disconnect: () => void;
}) {
  const [showSessions, setShowSessions] = useState(false);
  const [pairing, setPairing] = useState(false);
  const locked = disabled || Boolean(state.turn);
  const currentTitle = state.title || state.messages.find(({ role }) => role === "user")?.text.split("\n")[0] || state.turn?.messages[0]?.text.split("\n")[0] || "New conversation";

  return (
    <div className="layout">
      {pairing && <PairingDialog client={client} close={() => setPairing(false)} />}
      {showSessions && <button className="scrim" aria-label="Close conversations" onClick={() => setShowSessions(false)} />}
      <aside className={showSessions ? "rail rail-open" : "rail"}>
        <Brand />
        <div className="workspace-name"><Folder size={17} /><span>{state.workspace}</span></div>
        <button className="new-chat" disabled={locked} onClick={async () => {
          if (await client.send({ action: "new" })) setShowSessions(false);
        }}><Plus size={18} /> New conversation</button>
        <nav aria-label="Conversations">
          <h2>Conversations</h2>
          <button className="session selected" aria-current="page" onClick={() => setShowSessions(false)}>
            <MessageSquare size={15} /><span>{currentTitle}</span>
          </button>
          {state.sessions.map((session) => (
            <button key={session.id} className="session" disabled={locked || !session.readable} onClick={async () => {
              if (await client.send({ action: "resume", target: session.id })) setShowSessions(false);
            }}>
              <MessageSquare size={15} /><span>{session.title || session.id}</span>
            </button>
          ))}
        </nav>
        <div className="rail-actions">
          {state.mobilePairing && <button className="quiet" onClick={() => {
            setShowSessions(false);
            setPairing(true);
          }}><QrCode size={16} /> Pair a phone</button>}
          <button className="quiet" onClick={disconnect}><Unplug size={16} /> Disconnect</button>
        </div>
      </aside>
      <main className="main">
        <header className="topbar">
          <button className="icon menu" aria-label="Open conversations" aria-expanded={showSessions} onClick={() => setShowSessions(true)}><Menu size={20} /></button>
          <span className="current-title">{currentTitle}</span>
          <ModelSwitcher client={client} state={state} disabled={locked} />
        </header>
        {state.plan && <PlanPanel key={state.session} plan={state.plan} />}
        <Conversation key={state.session} client={client} state={state} error={error} disabled={disabled} />
      </main>
    </div>
  );
}

/** Switches the host's model between turns; models the host cannot use now are listed but disabled. */
function ModelSwitcher({ client, state, disabled }: { client: Client; state: WorkspaceState; disabled: boolean }) {
  if (!state.models.some(({ alias }) => alias !== state.model)) {
    return <span className="model"><CircleDot size={13} /> {state.model}</span>;
  }
  return (
    <label className="model" title={disabled ? "Switch models between turns." : undefined}>
      <CircleDot size={13} aria-hidden />
      <span className="visually-hidden">Model</span>
      <select value={state.model} disabled={disabled} onChange={(event) => {
        void client.send({ action: "set_model", model: event.currentTarget.value });
      }}>
        {state.models.some(({ alias }) => alias === state.model) ? null : <option value={state.model}>{state.model}</option>}
        {state.models.map((model) => (
          <option key={model.alias} value={model.alias} disabled={model.unavailable !== null}
            title={model.unavailable ?? model.description ?? undefined}>
            {model.alias} · {model.provider}{model.unavailable ? " (unavailable)" : ""}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Text with `code` spans, as the host writes approval options. */
function InlineCode({ text }: { text: string }) {
  return text.split(/`([^`]+)`/).map((part, index) => index % 2 ? <code key={index}>{part}</code> : part);
}

const planIcons: Record<PlanTaskStatus, typeof Circle> = {
  done: CircleCheck,
  active: LoaderCircle,
  pending: Circle,
  blocked: CircleAlert,
  dropped: CircleSlash,
};

/** The agent's plan, open by default except on small screens, where it would crowd the transcript. */
function PlanPanel({ plan }: { plan: Plan }) {
  const [open] = useState(() => !window.matchMedia("(max-width: 690px)").matches);
  const { done, total, active } = planProgress(plan);
  return (
    <details className="plan" open={open}>
      <summary>
        <ListChecks size={15} aria-hidden />
        <span className="plan-goal">{plan.goal}</span>
        {active && <span className="plan-active">· {active.title}</span>}
        <span className="plan-count">{done}/{total}</span>
      </summary>
      <progress value={plan.done} max={1000} aria-label={`Plan ${Math.round(plan.done / 10)}% done`} />
      <ol className="plan-tasks">
        {plan.tasks.map((task) => {
          const Icon = planIcons[task.status];
          return (
            <li key={task.id} className={`plan-task ${task.status}${task.group ? " group" : ""}`} style={{ "--depth": task.depth } as CSSProperties}>
              <Icon size={14} aria-hidden />
              <span>
                {task.title}
                <span className="visually-hidden"> ({planStatusLabels[task.status]})</span>
                {task.note && <small>{task.note}</small>}
              </span>
            </li>
          );
        })}
      </ol>
    </details>
  );
}

function Conversation({ client, state, error, disabled }: {
  client: Client;
  state: WorkspaceState;
  error: string | null;
  disabled: boolean;
}) {
  const [prompt, setPrompt] = useState("");
  const input = useRef<HTMLTextAreaElement>(null);
  const follow = useRef(true);
  const viewport = useRef<HTMLDivElement>(null);
  const watchContent = useCallback((node: HTMLDivElement | null) => {
    if (!node) return;
    const observer = new ResizeObserver(() => {
      const element = viewport.current;
      if (element && follow.current) element.scrollTop = element.scrollHeight;
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const turn = state.turn;
  const approval = turn?.approval;
  const messages = [...state.messages, ...(turn?.messages ?? [])];

  async function send(event: FormEvent) {
    event.preventDefault();
    if (prompt.trim() && !turn && await client.send({ action: "prompt", text: prompt })) {
      setPrompt("");
      follow.current = true;
      const element = viewport.current;
      if (element) element.scrollTop = element.scrollHeight;
    }
  }

  return (
    <>
      <div className="viewport" ref={viewport} onScroll={(event) => {
        const { scrollHeight, scrollTop, clientHeight } = event.currentTarget;
        follow.current = scrollHeight - scrollTop - clientHeight < 90;
      }}>
        <div ref={watchContent} className={messages.length ? "conversation" : "conversation empty"}>
          {!messages.length && (
            <section className="welcome">
              <span className="welcome-mark"><Terminal size={27} /></span>
              <h1>What are we working on?</h1>
              <p>Ask about your code, review changes, or give Socium a task.</p>
              <div className="starters">
                {starters.map(([label, text]) => (
                  <button key={label} className="quiet starter" disabled={disabled} onClick={() => {
                    setPrompt(text);
                    input.current?.focus();
                  }}>{label} <ArrowUp size={15} className="forward" /></button>
                ))}
              </div>
            </section>
          )}
          <div aria-label="Transcript">
            {messages.map((message, index) => <MessageCard key={index} message={message} />)}
            {turn?.draft && <MessageCard message={{ role: "assistant", text: turn.draft }} />}
          </div>
          {turn && !approval && <div className="running" role="status"><span className="pulse" aria-hidden />{describeActivity(turn.activity)}</div>}
          {state.notices.map((notice, index) => <p className="notice" key={index}>{notice}</p>)}
        </div>
      </div>
      <footer className="composer-area">
        {error && <p className="notice" role="alert">{error}</p>}
        {state.warnings.length > 0 && (
          <details className="notice">
            <summary>Host notices ({state.warnings.length})</summary>
            {state.warnings.map((warning, index) => <p key={index}>{warning}</p>)}
          </details>
        )}
        {turn && approval && (
          <section className="approval" aria-label="Approval required">
            <strong>Approval required · {approval.tool}</strong>
            <p>{approval.summary}{approval.requestedBy && ` · ${approval.requestedBy}`}</p>
            <pre tabIndex={0} aria-label="Tool call details">{approval.input}</pre>
            {approval.sessionOption && (
              <p id="session-option">Allow for session allows <InlineCode text={approval.sessionOption} /> until Socium exits.</p>
            )}
            <div className="approval-actions">
              <button className="quiet" disabled={disabled} onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: false })}>Deny</button>
              {approval.sessionOption && (
                <button className="quiet outlined" disabled={disabled} aria-describedby="session-option"
                  onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, scope: "session" })}>
                  Allow for session
                </button>
              )}
              <button className="primary" disabled={disabled} onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, scope: "once" })}>Allow once</button>
            </div>
          </section>
        )}
        <form className="composer" onSubmit={send}>
          <textarea
            ref={input}
            aria-label="Prompt"
            placeholder="Ask Socium anything…"
            value={prompt}
            rows={2}
            maxLength={maxPromptLength}
            onChange={(event) => setPrompt(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && !event.nativeEvent.isComposing) {
                event.currentTarget.form?.requestSubmit();
              }
            }}
          />
          {turn ? (
            <button type="button" className="icon stop" aria-label="Stop turn" disabled={disabled} onClick={() => void client.send({ action: "cancel", turn: turn.id })}><Square size={16} /></button>
          ) : (
            <button className="icon primary" aria-label="Send prompt" disabled={disabled || !prompt.trim()}><ArrowUp size={20} /></button>
          )}
        </form>
      </footer>
    </>
  );
}

function MessageCard({ message }: { message: Message }) {
  if (message.role === "tool" || message.role === "tool_error") {
    const [title, ...body] = message.text.split("\n");
    return <details className={message.role === "tool_error" ? "tool tool-error" : "tool"}>
      <summary>{title}</summary>
      {body.length > 0 && <pre>{body.join("\n")}</pre>}
    </details>;
  }
  const label = { user: "You", assistant: "Socium", system: "System" }[message.role];
  return <article className={`message ${message.role}`}>
    <h2>{label}</h2>
    {message.role === "assistant"
      ? <AssistantMarkdown text={message.text} />
      : <div className="plain">{message.text}</div>}
  </article>;
}
