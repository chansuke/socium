import type { Client } from "@socium/client";
import type { PairedDevice } from "@socium/client/protocol";

export interface PairingView {
  url: string | null;
  error: string | null;
  loading: boolean;
  devices: PairedDevice[] | null;
  devicesError: string | null;
  revoking: string | null;
  feedback: string | null;
}

const initialView: PairingView = {
  url: null, error: null, loading: false, devices: null, devicesError: null, revoking: null, feedback: null,
};

const errorMessage = (error: unknown, fallback: string) => error instanceof Error ? error.message : fallback;

/** Enough of the id to tell devices apart. */
export const shortId = (id: string) => id.slice(0, 8);

type PairingClient = Pick<Client, "mobilePairingUrl" | "listDevices" | "revokeDevice">;

/**
 * The requests and state of one open pairing dialog. `close` aborts every
 * request in flight, and nothing that finishes afterwards changes the state
 * or sends another request. `open` starts again after a close, as when React
 * attaches the dialog a second time in development.
 */
export function createPairingSession(client: PairingClient) {
  let controller = new AbortController();
  let view = initialView;
  const listeners = new Set<() => void>();
  /** Only the latest device list request may update the list. */
  let listing = 0;

  function set(signal: AbortSignal, changes: Partial<PairingView>) {
    if (signal.aborted) return;
    view = { ...view, ...changes };
    for (const listener of listeners) listener();
  }

  /** Lists the devices for the dialog session that `signal` belongs to. */
  async function loadDevices(signal = controller.signal) {
    if (signal.aborted) return;
    const request = ++listing;
    try {
      const devices = await client.listDevices(signal);
      if (request === listing) set(signal, { devices, devicesError: null });
    } catch (error) {
      if (request === listing) set(signal, { devicesError: errorMessage(error, "Could not load paired devices.") });
    }
  }

  return {
    getSnapshot: () => view,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    open() {
      if (!controller.signal.aborted) return;
      controller = new AbortController();
      view = initialView;
    },
    close() {
      controller.abort(new DOMException("The pairing dialog closed.", "AbortError"));
    },
    loadDevices: () => loadDevices(),
    async issue() {
      const { signal } = controller;
      if (signal.aborted) return;
      set(signal, { error: null, url: null, loading: true });
      try {
        const url = await client.mobilePairingUrl(signal);
        set(signal, { url });
      } catch (error) {
        set(signal, { error: errorMessage(error, "Could not create a QR code.") });
      } finally {
        set(signal, { loading: false });
      }
      // A pairing finishes on the phone; the list shows it after the next refresh.
      await loadDevices(signal);
    },
    async revoke(device: PairedDevice) {
      const { signal } = controller;
      if (signal.aborted) return;
      set(signal, { revoking: device.id, feedback: null, devicesError: null });
      try {
        await client.revokeDevice(device.id, signal);
        set(signal, { feedback: `Revoked device ${shortId(device.id)}. It must pair again to connect.` });
      } catch (error) {
        set(signal, { devicesError: errorMessage(error, "Could not revoke the device.") });
      } finally {
        set(signal, { revoking: null });
      }
      await loadDevices(signal);
    },
    /** For the QR canvas begun under `signal`, which may fail after the dialog closed. */
    drawFailed(signal: AbortSignal) {
      set(signal, { error: "Could not draw the QR code." });
    },
    get signal() {
      return controller.signal;
    },
  };
}

export type PairingSession = ReturnType<typeof createPairingSession>;

/**
 * Handlers for the dialog element. Closing with the button or Escape ends the
 * session at once, in the same event: the dialog's `close` event comes in a
 * later task, when a reply may already have changed the state. `closed`
 * runs on that `close` event.
 */
export function dialogHandlers(session: Pick<PairingSession, "close">, closed: () => void) {
  return {
    /** For the Close button, which submits the dialog's form. */
    onCloseClick: () => session.close(),
    /** For Escape. */
    onCancel: () => session.close(),
    onClose: () => {
      session.close();
      closed();
    },
  };
}
