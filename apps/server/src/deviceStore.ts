import { randomBytes } from "node:crypto";
import {
  closeSync, constants, fchmodSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync,
  unlinkSync, writeSync,
} from "node:fs";
import type { Stats } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import type { PairedDevice } from "@socium/client/protocol";

export interface DeviceRecord extends PairedDevice {
  /** SHA-256 of the device token, hex. The token itself is never stored. */
  tokenHash: string;
}

/** The records in the store and the version of the file they came from. */
export interface StoreSnapshot {
  devices: DeviceRecord[];
  version: string;
}

/**
 * The user-private file that keeps paired devices across restarts. A relative
 * `XDG_STATE_HOME` is ignored, as the XDG Base Directory specification asks.
 */
export function defaultPairingStore() {
  const state = process.env.XDG_STATE_HOME;
  const root = state && isAbsolute(state) ? state : join(homedir(), ".local/state");
  return join(root, "socium", "paired-devices.json");
}

/** `path` with its existing ancestors resolved through symlinks. */
function realLocation(path: string): string {
  try {
    return realpathSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    const parent = dirname(path);
    return parent === path ? path : join(realLocation(parent), basename(path));
  }
}

/**
 * Rejects a store inside the workspace, where the agent's tools can read and
 * write it. `workspace` must already be a real path.
 */
export function checkStoreLocation(storePath: string, workspace: string) {
  const store = realLocation(resolve(storePath));
  const inside = relative(workspace, store);
  if (inside === "" || (!inside.startsWith("..") && !isAbsolute(inside))) {
    throw new Error(`Keep the paired device store outside the workspace: ${store} is inside ${workspace}.`);
  }
}

const currentUser = () => process.getuid?.();

function checkPrivate(stats: Stats, path: string, kind: "directory" | "file") {
  const mode = kind === "directory" ? 0o700 : 0o600;
  const uid = currentUser();
  if (kind === "directory" ? !stats.isDirectory() : !stats.isFile()) {
    throw new Error(`The paired device store ${path} must be a ${kind}, not a symlink or other file.`);
  }
  if (uid !== undefined && stats.uid !== uid) {
    throw new Error(`The paired device store ${path} must belong to the user running the gateway.`);
  }
  if ((stats.mode & 0o777) !== mode) {
    throw new Error(`The paired device store ${path} must have mode ${mode.toString(8)}; run chmod ${mode.toString(8)} ${path}.`);
  }
}

/** Creates the store directory if needed and checks that only this user can use it. */
function checkDirectory(path: string, create: boolean) {
  const directory = dirname(path);
  if (create) mkdirSync(directory, { recursive: true, mode: 0o700 });
  let stats: Stats;
  try {
    stats = lstatSync(directory);
  } catch (error) {
    if (!create && (error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  checkPrivate(stats, directory, "directory");
  return true;
}

/** Changes with the contents, a replacement, or a change of owner or mode. */
function versionOf(stats: Stats) {
  return `${stats.dev}:${stats.ino}:${stats.size}:${stats.mtimeMs}:${stats.ctimeMs}`;
}

/** The version of the file now, without reading it; "missing" before the first pairing. */
export function storeVersion(path: string) {
  try {
    return versionOf(lstatSync(path));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "missing";
    throw error;
  }
}

function isRecord(value: unknown): value is DeviceRecord {
  const record = value as DeviceRecord | null;
  return typeof record?.id === "string" && typeof record.createdAt === "string" &&
    (record.lastUsedAt === null || typeof record.lastUsedAt === "string") &&
    typeof record.tokenHash === "string" && /^[0-9a-f]{64}$/.test(record.tokenHash);
}

/** Reads the store without following a symlink, and checks it before trusting it. */
export function readStore(path: string): StoreSnapshot {
  if (!checkDirectory(path, false)) return { devices: [], version: "missing" };
  let descriptor: number;
  try {
    descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT") return { devices: [], version: "missing" };
    if (code === "ELOOP") {
      throw new Error(`The paired device store ${path} must be a file, not a symlink or other file.`, { cause: error });
    }
    throw error;
  }
  let text: string;
  let version: string;
  try {
    const stats = fstatSync(descriptor);
    checkPrivate(stats, path, "file");
    version = versionOf(stats);
    text = readFileSync(descriptor, "utf8");
  } finally {
    closeSync(descriptor);
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
  return {
    devices: devices.map(({ id, createdAt, lastUsedAt, tokenHash }) => ({ id, createdAt, lastUsedAt, tokenHash })),
    version,
  };
}

/** Replaces the store in one step, so a crash leaves the old file or the new one. */
export function writeStore(path: string, devices: DeviceRecord[]): StoreSnapshot {
  checkDirectory(path, true);
  const temporary = `${path}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
  const descriptor = openSync(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    let version: string;
    try {
      // The umask may have taken away more than group and other bits.
      fchmodSync(descriptor, 0o600);
      writeSync(descriptor, JSON.stringify({ devices }, null, 2) + "\n");
      fsyncSync(descriptor);
      version = versionOf(fstatSync(descriptor));
    } finally {
      closeSync(descriptor);
    }
    renameSync(temporary, path);
    return { devices, version };
  } catch (error) {
    try {
      unlinkSync(temporary);
    } catch {
      // Already renamed or never written.
    }
    throw error;
  }
}
