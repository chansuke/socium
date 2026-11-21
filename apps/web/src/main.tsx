import { StrictMode, useCallback, useRef, useState, useSyncExternalStore } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { ArrowUp, ChevronLeft, CircleDot, Folder, Menu, MessageSquare, Plus, QrCode, Square, Terminal, Unplug } from "lucide-react";
import Markdown from "react-markdown";
import { createClient } from "@socium/client";
import type { Client } from "@socium/client";
import { maxPromptLength } from "@socium/client/protocol";
import type { Message, WorkspaceState } from "@socium/client/protocol";
import { PairingDialog } from "./PairingDialog";
import "./styles.css";

const starters = [
  ["Explore the workspace", "Explain the structure of this repository and its main entry points."],
  ["Review local changes", "Review the current changes and flag bugs or missing tests."],
  ["Plan a fix", "Suggest a plan to fix an issue. Ask me for the details first."],
];

function Brand() {
  return <span className="brand"><span className="mark"><Terminal size={19} /></span>socium</span>;
}

function App() {
  const [client, setClient] = useState<Client | null>(null);
  if (client) return <Connection client={client} disconnect={() => setClient(null)} />;

  function connect(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = String(new FormData(event.currentTarget).get("token")).trim();
    if (token) setClient(createClient("", token));
  }

  return (
    <main className="connect">
      <form className="connect-card" onSubmit={connect}>
        <Brand />
        <h1>Connect to Socium</h1>
        <p>Enter the access token shown in the terminal on your Socium host.</p>
        <label htmlFor="token">Access token</label>
        <input id="token" name="token" type="password" autoComplete="off" required />
        <button className="primary">Connect <ArrowUp size={17} className="forward" /></button>
      </form>
    </main>
  );
}

function Connection({ client, disconnect }: { client: Client; disconnect: () => void }) {
  const status = useSyncExternalStore(client.subscribe, client.getSnapshot);
  if (!status.workspace) {
    return (
      <main className="connect">
        <div className="connect-card">
          <Brand />
          <h1>{status.connectionError ? "Connection unavailable" : "Connecting…"}</h1>
          {status.connectionError && <p role="alert">{status.connectionError}</p>}
          <button className="quiet" onClick={disconnect}><ChevronLeft size={17} /> Back to connection</button>
        </div>
      </main>
    );
  }

  return (
    <Workspace
      client={client}
      state={status.workspace}
      error={status.connectionError ?? status.commandError}
      disabled={status.sending || Boolean(status.connectionError)}
      disconnect={disconnect}
    />
  );
}

function Workspace({
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
          <span className="model"><CircleDot size={13} /> {state.model}</span>
        </header>
        <Conversation key={state.session} client={client} state={state} error={error} disabled={disabled} />
      </main>
    </div>
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
          {turn && !approval && <div className="running" role="status">Running…</div>}
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
            {approval.sessionOption && <p>Allow for session also allows {approval.sessionOption} until Socium exits.</p>}
            <div className="approval-actions">
              <button className="quiet" disabled={disabled} onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: false })}>Deny</button>
              {approval.sessionOption && (
                <button className="quiet" disabled={disabled} title={`Also allow ${approval.sessionOption} until Socium exits`} onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, forSession: true })}>Allow for session</button>
              )}
              <button className="primary" disabled={disabled} onClick={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true })}>Allow once</button>
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
      ? <Markdown skipHtml disallowedElements={["img"]} components={{
        a: ({ href, children }) => <a href={href} target="_blank" rel="noreferrer noopener">{children}</a>,
      }}>{message.text}</Markdown>
      : <div className="plain">{message.text}</div>}
  </article>;
}

const root = document.querySelector("#root");
if (!root) throw new Error("Missing Socium root element.");
createRoot(root).render(<StrictMode><App /></StrictMode>);
