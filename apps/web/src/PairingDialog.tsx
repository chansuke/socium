import { useCallback, useState } from "react";
import { toCanvas } from "qrcode";
import type { Client } from "@socium/client";

export function PairingDialog({ client, close }: { client: Client; close: () => void }) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const drawCode = useCallback((canvas: HTMLCanvasElement | null) => {
    if (canvas && url) {
      void toCanvas(canvas, url, { width: 256, margin: 4 }).catch(() => {
        setError("Could not draw the QR code.");
      });
    }
  }, [url]);

  async function issue() {
    setError(null);
    setUrl(null);
    setLoading(true);
    try {
      setUrl(await client.mobilePairingUrl());
    } catch (error) {
      setError(error instanceof Error ? error.message : "Could not create a QR code.");
    } finally {
      setLoading(false);
    }
  }

  return (
    <dialog className="pairing-dialog" aria-labelledby="pairing-heading" onClose={close} ref={(dialog) => {
      if (dialog && !dialog.open) dialog.showModal();
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
        <form method="dialog"><button className="quiet">Close</button></form>
      </div>
    </dialog>
  );
}
