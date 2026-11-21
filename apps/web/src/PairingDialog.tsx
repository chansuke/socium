import { useCallback, useRef, useState } from "react";
import { toCanvas } from "qrcode";
import { RefreshCw, Smartphone } from "lucide-react";
import type { Client } from "@socium/client";
import type { PairedDevice } from "@socium/client/protocol";

const errorMessage = (error: unknown, fallback: string) => error instanceof Error ? error.message : fallback;

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function formatTime(value: string | null) {
  if (value === null) return "never";
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? value : dateFormat.format(time);
}

/** Enough of the id to tell devices apart. */
const shortId = (id: string) => id.slice(0, 8);

export function PairingDialog({ client, close }: { client: Client; close: () => void }) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [devices, setDevices] = useState<PairedDevice[] | null>(null);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  /** Only the latest device list request may update the list. */
  const listing = useRef(0);
  const drawCode = useCallback((canvas: HTMLCanvasElement | null) => {
    if (canvas && url) {
      void toCanvas(canvas, url, { width: 256, margin: 4 }).catch(() => {
        setError("Could not draw the QR code.");
      });
    }
  }, [url]);

  async function loadDevices() {
    const request = ++listing.current;
    try {
      const listed = await client.listDevices();
      if (request !== listing.current) return;
      setDevices(listed);
      setDevicesError(null);
    } catch (error) {
      if (request === listing.current) setDevicesError(errorMessage(error, "Could not load paired devices."));
    }
  }

  async function issue() {
    setError(null);
    setUrl(null);
    setLoading(true);
    try {
      setUrl(await client.mobilePairingUrl());
    } catch (error) {
      setError(errorMessage(error, "Could not create a QR code."));
    } finally {
      setLoading(false);
    }
    // A pairing finishes on the phone; the list shows it after the next refresh.
    void loadDevices();
  }

  async function revoke(device: PairedDevice) {
    setRevoking(device.id);
    setFeedback(null);
    setDevicesError(null);
    try {
      await client.revokeDevice(device.id);
      setFeedback(`Revoked device ${shortId(device.id)}. It must pair again to connect.`);
    } catch (error) {
      setDevicesError(errorMessage(error, "Could not revoke the device."));
    } finally {
      setRevoking(null);
    }
    await loadDevices();
  }

  return (
    <dialog className="pairing-dialog" aria-labelledby="pairing-heading" onClose={close} ref={(dialog) => {
      if (dialog && !dialog.open) {
        dialog.showModal();
        void loadDevices();
      }
    }}>
      <h2 id="pairing-heading">Pair a phone</h2>
      <p>Open Socium on your phone and choose <strong>Scan QR code</strong>.</p>
      {url && <canvas ref={drawCode} role="img" aria-label="Socium mobile pairing QR code" />}
      {url && <p>This code expires after five minutes or one scan.</p>}
      {error && <p className="notice" role="alert">{error}</p>}
      <div className="pairing-actions">
        <button className="primary" disabled={loading} onClick={() => void issue()}>
          {loading ? "Generating…" : url ? "Generate new code" : "Generate QR code"}
        </button>
      </div>

      <section className="devices" aria-labelledby="devices-heading">
        <div className="devices-header">
          <h3 id="devices-heading">Paired devices</h3>
          <button className="icon small" aria-label="Refresh paired devices" title="Refresh" onClick={() => void loadDevices()}>
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
                  onClick={() => void revoke(device)}>
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
