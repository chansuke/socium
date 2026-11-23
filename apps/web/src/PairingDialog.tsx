import { useCallback, useState, useSyncExternalStore } from "react";
import { toCanvas } from "qrcode";
import { RefreshCw, Smartphone } from "lucide-react";
import type { Client } from "@socium/client";
import { createPairingSession, shortId } from "./pairingSession";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function formatTime(value: string | null) {
  if (value === null) return "never";
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? value : dateFormat.format(time);
}

export function PairingDialog({ client, close }: { client: Client; close: () => void }) {
  const [session] = useState(() => createPairingSession(client));
  const { url, error, loading, devices, devicesError, revoking, feedback } = useSyncExternalStore(session.subscribe, session.getSnapshot);
  const drawCode = useCallback((canvas: HTMLCanvasElement | null) => {
    const { signal } = session;
    if (canvas && url && !signal.aborted) {
      void toCanvas(canvas, url, { width: 256, margin: 4 }).catch(() => session.drawFailed(signal));
    }
  }, [session, url]);
  // Stable, so React detaches it only when the dialog unmounts; that aborts what is in flight.
  const attach = useCallback((dialog: HTMLDialogElement | null) => {
    if (!dialog) return;
    session.open();
    if (!dialog.open) dialog.showModal();
    void session.loadDevices();
    return () => session.close();
  }, [session]);

  return (
    <dialog className="pairing-dialog" aria-labelledby="pairing-heading" ref={attach} onClose={() => {
      session.close();
      close();
    }}>
      <h2 id="pairing-heading">Pair a phone</h2>
      <p>Open Socium on your phone and choose <strong>Scan QR code</strong>.</p>
      {url && <canvas ref={drawCode} role="img" aria-label="Socium mobile pairing QR code" />}
      {url && <p>This code expires after five minutes or one scan.</p>}
      {error && <p className="notice" role="alert">{error}</p>}
      <div className="pairing-actions">
        <button className="primary" disabled={loading} onClick={() => void session.issue()}>
          {loading ? "Generating…" : url ? "Generate new code" : "Generate QR code"}
        </button>
      </div>

      <section className="devices" aria-labelledby="devices-heading">
        <div className="devices-header">
          <h3 id="devices-heading">Paired devices</h3>
          <button className="icon small" aria-label="Refresh paired devices" title="Refresh" onClick={() => void session.loadDevices()}>
            <RefreshCw size={15} />
          </button>
        </div>
        <p className="device-feedback" role="status">{feedback}</p>
        {devicesError && <p className="notice" role="alert">{devicesError}</p>}
        {devices === null ? (
          !devicesError && <p>Loading paired devices…</p>
        ) : devices.length === 0 ? (
          <p>No phones are paired.</p>
        ) : (
          <ul className="device-list">
            {devices.map((device) => (
              <li key={device.id}>
                <Smartphone size={17} aria-hidden />
                <div>
                  <strong title={device.id}>Device {shortId(device.id)}</strong>
                  <span>Paired {formatTime(device.createdAt)}</span>
                  <span>Last used {formatTime(device.lastUsedAt)}</span>
                </div>
                <button className="quiet danger" disabled={revoking !== null} aria-label={`Revoke device ${shortId(device.id)}`}
                  onClick={() => void session.revoke(device)}>
                  {revoking === device.id ? "Revoking…" : "Revoke"}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <form method="dialog" className="dialog-close"><button className="quiet">Close</button></form>
    </dialog>
  );
}
