import { createHash, randomBytes, randomUUID } from "node:crypto";
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, statSync, unlinkSync, writeSync } from "node:fs";
import type { Stats } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { mobileEndpoint } from "@socium/client";
import type { PairedDevice } from "@socium/client/protocol";

const lifetime = 5 * 60_000;
/** `lastUsedAt` moves in steps of this much, so polling does not rewrite the store. */
const usageResolution = 60_000;

interface DeviceRecord extends PairedDevice {
  /** SHA-256 of the device token, hex. The token itself is never stored. */
  tokenHash: string;
}

function hash(value: string) {
  return createHash("sha256").update(value).digest("hex");
}

/** The user-private file that keeps paired devices across restarts, outside every workspace. */
export function defaultPairingStore() {
  return join(process.env.XDG_STATE_HOME || join(homedir(), ".local/state"), "socium", "paired-devices.json");
}

function isRecord(value: unknown): value is DeviceRecord {
  const record = value as DeviceRecord | null;
  return typeof record?.id === "string" && typeof record.createdAt === "string" &&
    (record.lastUsedAt === null || typeof record.lastUsedAt === "string") &&
    typeof record.tokenHash === "string" && /^[0-9a-f]{64}$/.test(record.tokenHash);
}

function readRecords(path: string): DeviceRecord[] {
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    data = null;
  }
  const devices = (data as { devices?: unknown } | null)?.devices;
  if (!Array.isArray(devices) || !devices.every(isRecord)) {
    throw new Error(`The paired device store ${path} is damaged. Fix or remove it, then pair devices again.`);
  }
  return devices.map(({ id, createdAt, lastUsedAt, tokenHash }) => ({ id, createdAt, lastUsedAt, tokenHash }));
}

/** Replaces the store in one step, so a crash leaves the old file or the new one. */
function writeRecords(path: string, devices: DeviceRecord[]) {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
  const descriptor = openSync(temporary, "wx", 0o600);
  try {
    try {
      writeSync(descriptor, JSON.stringify({ devices }, null, 2) + "\n");
      fsyncSync(descriptor);
    } finally {
      closeSync(descriptor);
    }
    renameSync(temporary, path);
  } catch (error) {
    try {
      unlinkSync(temporary);
    } catch {
      // Already renamed or never written.
    }
    throw error;
  }
}

function fileVersion(path: string) {
  let stats: Stats;
  try {
    stats = statSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "missing";
    throw error;
  }
  return `${stats.ino}:${stats.size}:${stats.mtimeMs}`;
}

export interface PairingOptions {
  clock?: () => number;
  /** Where paired devices are kept; defaults to `defaultPairingStore()`. */
  storePath?: string;
}

export function createPairing(address: string, { clock = Date.now, storePath = defaultPairingStore() }: PairingOptions = {}) {
  const origin = mobileEndpoint(address);
  let devices = readRecords(storePath);
  let version = fileVersion(storePath);
  let pending: { hash: string; expires: number } | undefined;

  /** Picks up changes another gateway made, such as a revocation. */
  function load() {
    const current = fileVersion(storePath);
    if (current === version) return;
    devices = readRecords(storePath);
    version = current;
  }

  function save(next: DeviceRecord[]) {
    writeRecords(storePath, next);
    devices = next;
    version = fileVersion(storePath);
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
      load();
      const device = { id: randomUUID(), createdAt: new Date(clock()).toISOString(), lastUsedAt: null, tokenHash: hash(token) };
      save([...devices, device]);
      return token;
    },
    authorizes(authorization: string) {
      if (!authorization.startsWith("Bearer ")) return false;
      load();
      const tokenHash = hash(authorization.slice("Bearer ".length));
      const device = devices.find((record) => record.tokenHash === tokenHash);
      if (!device) return false;
      const now = clock();
      if (device.lastUsedAt === null || now - Date.parse(device.lastUsedAt) >= usageResolution) {
        const lastUsedAt = new Date(now).toISOString();
        save(devices.map((record) => record === device ? { ...record, lastUsedAt } : record));
      }
      return true;
    },
    /** Paired devices, oldest first, without their token hashes. */
    devices(): PairedDevice[] {
      load();
      return devices.map(({ id, createdAt, lastUsedAt }) => ({ id, createdAt, lastUsedAt }));
    },
    /** Forgets a device; its token stops working at once. */
    revoke(id: string) {
      load();
      if (!devices.some((record) => record.id === id)) return false;
      save(devices.filter((record) => record.id !== id));
      return true;
    },
  };
}

export type Pairing = ReturnType<typeof createPairing>;
