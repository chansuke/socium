import { StrictMode, useState, useSyncExternalStore } from "react";
import type { FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { ArrowUp, ChevronLeft } from "lucide-react";
import { createClient } from "@socium/client";
import type { Client } from "@socium/client";
import { Brand, Workspace } from "./Workspace";
import "./styles.css";

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
      error={status.connectionError ?? (status.reconnecting ? "Reconnecting to the Socium host…" : status.commandError)}
      disabled={status.sending || status.reconnecting || Boolean(status.connectionError)}
      disconnect={disconnect}
    />
  );
}

const root = document.querySelector("#root");
if (!root) throw new Error("Missing Socium root element.");
createRoot(root).render(<StrictMode><App /></StrictMode>);
