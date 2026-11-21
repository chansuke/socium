import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mobileEndpoint } from "@socium/client";
import type { PairedDevice } from "@socium/client/protocol";
import { checkStoreLocation, defaultPairingStore, readStore, storeVersion, writeStore } from "./deviceStore.ts";
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
}

export function createPairing(address: string, { clock = Date.now, storePath = defaultPairingStore(), workspace }: PairingOptions = {}) {
  const origin = mobileEndpoint(address);
  if (workspace !== undefined) checkStoreLocation(storePath, workspace);
  /** The records of the file version last read or written here. */
  let store: StoreSnapshot = readStore(storePath);
  let pending: { hash: string; expires: number } | undefined;

  /** Picks up changes another gateway made, such as a revocation. */
  function load() {
    if (storeVersion(storePath) !== store.version) store = readStore(storePath);
    return store.devices;
  }

  function save(next: DeviceRecord[]) {
    store = writeStore(storePath, next);
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
      save([...load(), device]);
      return token;
    },
    authorizes(authorization: string) {
      if (!authorization.startsWith("Bearer ")) return false;
      const tokenHash = hash(authorization.slice("Bearer ".length));
      const now = clock();
      const devices = load();
      const device = devices.find((record) => record.tokenHash === tokenHash);
      if (!device) return false;
      if (device.lastUsedAt === null || now - Date.parse(device.lastUsedAt) >= usageResolution) {
        const lastUsedAt = new Date(now).toISOString();
        save(devices.map((record) => record === device ? { ...record, lastUsedAt } : record));
      }
      return true;
    },
    /** Paired devices, oldest first, without their token hashes. */
    devices(): PairedDevice[] {
      return load().map(({ id, createdAt, lastUsedAt }) => ({ id, createdAt, lastUsedAt }));
    },
    /** Forgets a device; its token stops working at once. */
    revoke(id: string) {
      const devices = load();
      if (!devices.some((record) => record.id === id)) return false;
      save(devices.filter((record) => record.id !== id));
      return true;
    },
  };
}

export type Pairing = ReturnType<typeof createPairing>;
