import { createHash, randomBytes, randomUUID } from "node:crypto";
import { watch } from "node:fs";
import { basename, dirname } from "node:path";
import { mobileEndpoint } from "@socium/client";
import type { PairedDevice } from "@socium/client/protocol";
import { defaultPairingStore, openStore } from "./deviceStore.ts";
import type { DeviceRecord, StoreSnapshot } from "./deviceStore.ts";

export { defaultPairingStore } from "./deviceStore.ts";

const lifetime = 5 * 60_000;
/** `lastUsedAt` moves in steps of this much, so polling does not rewrite the store. */
const usageResolution = 60_000;

function hash(value: string) {
  return createHash("sha256").update(value).digest("hex");
}

export interface PairingOptions {
  clock?: () => number;
  /** Where paired devices are kept; defaults to `defaultPairingStore()`. */
  storePath?: string;
  /** The real path of the served workspace, which must not contain the store. */
  workspace?: string;
  /** How long a store update waits for another gateway's update, in milliseconds. */
  lockTimeout?: number;
}

export function createPairing(
  address: string,
  { clock = Date.now, storePath = defaultPairingStore(), workspace, lockTimeout }: PairingOptions = {},
) {
  const origin = mobileEndpoint(address);
  const file = openStore(storePath, { workspace, lockTimeout });
  /** The records of the file version last read or written here. */
  let store: StoreSnapshot = file.read();
  let pending: { hash: string; expires: number } | undefined;

  /** Picks up changes another gateway made, such as a revocation. */
  function load() {
    if (file.version() !== store.version) store = file.read();
    return store.devices;
  }

  /** Changes the latest records on disk, never the cached ones, under the store lock. */
  function update(change: (devices: DeviceRecord[]) => DeviceRecord[] | null) {
    store = file.update(change);
    return store.devices;
  }

  /** The id of the paired device that `authorization` belongs to, or null. */
  function authenticate(authorization: string): string | null {
    if (!authorization.startsWith("Bearer ")) return null;
    const tokenHash = hash(authorization.slice("Bearer ".length));
    const now = clock();
    const due = (record: DeviceRecord) =>
      record.lastUsedAt === null || now - Date.parse(record.lastUsedAt) >= usageResolution;
    const cached = load().find((record) => record.tokenHash === tokenHash);
    if (!cached) return null;
    if (!due(cached)) return cached.id;
    const lastUsedAt = new Date(now).toISOString();
    // Decided again on the locked file: another gateway may have revoked it.
    const devices = update((current) => {
      const device = current.find((record) => record.tokenHash === tokenHash);
      return device && due(device) ? current.map((record) => record === device ? { ...record, lastUsedAt } : record) : null;
    });
    return devices.find((record) => record.tokenHash === tokenHash)?.id ?? null;
  }

  return {
    storePath,
    issue() {
      const code = randomBytes(32).toString("base64url");
      pending = { hash: hash(code), expires: clock() + lifetime };
      return `${origin}/pair#${code}`;
    },
    redeem(code: string) {
      if (!pending || clock() >= pending.expires || pending.hash !== hash(code)) return null;
      pending = undefined;
      const token = randomBytes(32).toString("base64url");
      const device = { id: randomUUID(), createdAt: new Date(clock()).toISOString(), lastUsedAt: null, tokenHash: hash(token) };
      update((devices) => [...devices, device]);
      return token;
    },
    authenticate,
    authorizes: (authorization: string) => authenticate(authorization) !== null,
    /** Paired devices, oldest first, without their token hashes. */
    devices(): PairedDevice[] {
      return load().map(({ id, createdAt, lastUsedAt }) => ({ id, createdAt, lastUsedAt }));
    },
    /** Whether a device is still paired, as the latest store says; unlike `authenticate`, not a use. */
    isPaired: (id: string) => load().some((record) => record.id === id),
    /**
     * Calls `onChange` soon after any gateway on this store replaces the file,
     * such as for a revocation, or when watching fails. Returns a function
     * that stops watching. The directory is watched because each update
     * replaces the file.
     */
    watch(onChange: () => void) {
      const name = basename(storePath);
      const watcher = watch(dirname(storePath), { persistent: false }, (_event, changed) => {
        if (changed === null || changed === name) onChange();
      });
      watcher.on("error", onChange);
      return () => watcher.close();
    },
    /** Forgets a device; its token stops working at once. */
    revoke(id: string) {
      let found = false;
      update((devices) => {
        found = devices.some((record) => record.id === id);
        return found ? devices.filter((record) => record.id !== id) : null;
      });
      return found;
    },
  };
}

export type Pairing = ReturnType<typeof createPairing>;
