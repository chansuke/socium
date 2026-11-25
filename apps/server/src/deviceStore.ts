import { randomBytes } from "node:crypto";
import {
  closeSync, constants, fchmodSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync,
  unlinkSync, writeSync,
} from "node:fs";
import type { Stats } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { DatabaseSync } from "node:sqlite";
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

/** How long an update waits for another gateway's update before failing. */
const defaultLockTimeout = 5000;
/** SQLite's result code for a database another connection has locked. */
const sqliteBusy = 5;

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

/** Names the private file in error messages. */
const storeLabel = "paired device store";

/**
 * Rejects a store inside the workspace, where the agent's tools can read and
 * write it. `workspace` must already be a real path.
 */
export function checkStoreLocation(storePath: string, workspace: string, label = storeLabel) {
  const store = realLocation(resolve(storePath));
  const fromWorkspace = relative(workspace, store);
  // Only `..` as a whole segment leaves it: `<workspace>/..hidden` is inside.
  const outside = fromWorkspace === ".." || fromWorkspace.startsWith(`..${sep}`) || isAbsolute(fromWorkspace);
  if (!outside) {
    throw new Error(`Keep the ${label} outside the workspace: ${store} is inside ${workspace}.`);
  }
}

const currentUser = () => process.getuid?.();

function checkPrivate(stats: Stats, path: string, kind: "directory" | "file", label = storeLabel) {
  const mode = kind === "directory" ? 0o700 : 0o600;
  const uid = currentUser();
  if (kind === "directory" ? !stats.isDirectory() : !stats.isFile()) {
    throw new Error(`The ${label} ${path} must be a ${kind}, not a symlink or other file.`);
  }
  if (uid !== undefined && stats.uid !== uid) {
    throw new Error(`The ${label} ${path} must belong to the user running the gateway.`);
  }
  // Setuid, setgid, and sticky bits count too: the mode must be exactly this.
  if ((stats.mode & 0o7777) !== mode) {
    throw new Error(`The ${label} ${path} must have mode ${mode.toString(8)}; run chmod ${mode.toString(8)} ${path}.`);
  }
}

/**
 * Changes with the contents, a replacement, or a change of owner or mode.
 * The owner and mode are part of it because file times can stay the same
 * across changes made within one clock tick.
 */
function versionOf(stats: Stats) {
  return `${stats.dev}:${stats.ino}:${stats.size}:${stats.mtimeMs}:${stats.ctimeMs}:${stats.mode}:${stats.uid}`;
}

/** The version of the file now, without reading it; "missing" before the first pairing. */
function fileVersion(path: string) {
  let stats: Stats;
  try {
    stats = lstatSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "missing";
    throw error;
  }
  checkPrivate(stats, path, "file");
  return versionOf(stats);
}

function isRecord(value: unknown): value is DeviceRecord {
  const record = value as DeviceRecord | null;
  return typeof record?.id === "string" && typeof record.createdAt === "string" &&
    (record.lastUsedAt === null || typeof record.lastUsedAt === "string") &&
    typeof record.tokenHash === "string" && /^[0-9a-f]{64}$/.test(record.tokenHash);
}

/** Reads the store without following a symlink, and checks it before trusting it. */
function readFile(path: string): StoreSnapshot {
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
function writeStore(path: string, devices: DeviceRecord[]): StoreSnapshot {
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

/**
 * Whether another user could rename or replace entries in the directory
 * `stats` describes, when the gateway runs as `uid`. It must belong to this
 * user or root, and no other user may write to it. A root-owned sticky
 * directory such as `/tmp` may be writable by all, because others can add
 * entries to it but cannot move or remove ours.
 */
export function othersCanReplace(stats: Pick<Stats, "isDirectory" | "mode" | "uid">, uid: number) {
  const owned = stats.uid === uid || stats.uid === 0;
  const sharedWrite = (stats.mode & 0o022) !== 0 && !(stats.uid === 0 && (stats.mode & 0o1000) !== 0);
  return !stats.isDirectory() || !owned || sharedWrite;
}

/**
 * Checks every directory from `directory` up to `/` with `othersCanReplace`,
 * so nobody else can rename or replace a directory on the way to `file`
 * between one check and the next. `directory` must already be a real path.
 */
function checkAncestors(directory: string, file: string, label: string) {
  const uid = currentUser();
  if (uid === undefined) return;
  for (let current = directory; ; current = dirname(current)) {
    if (othersCanReplace(lstatSync(current), uid)) {
      throw new Error(
        `The ${label} ${file} is under ${current}, which another user could replace; ` +
          "keep it under directories that only you or root can change.",
      );
    }
    if (dirname(current) === current) return;
  }
}

/** The real path of the deepest ancestor of `directory` that exists. */
function existingAncestor(directory: string): string {
  for (let current = directory; ; current = dirname(current)) {
    try {
      lstatSync(current);
      return realpathSync(current);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT" || dirname(current) === current) throw error;
    }
  }
}

function isMissing(path: string) {
  try {
    lstatSync(path);
    return false;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return true;
    throw error;
  }
}

export interface PrivateLocationOptions {
  /** Whether to create the directory with mode 700 when it is missing. */
  create: boolean;
  label?: string;
  /** The real path of the served workspace, which must not contain the file. */
  workspace?: string;
}

/**
 * Checks where the private file at `path` lives and returns its path in the
 * real directory, or null when the directory is missing and `create` is
 * false. The file must be outside the workspace, its directory must be a
 * private directory of this user and not a symlink, and no directory from
 * there up to `/` may be one that another user could replace. A missing
 * directory is created only after the ancestors that exist pass, so a refused
 * path leaves nothing behind.
 */
export function privateLocation(path: string, options: PrivateLocationOptions & { create: true }): string;
export function privateLocation(path: string, options: PrivateLocationOptions): string | null;
export function privateLocation(path: string, { create, label = storeLabel, workspace }: PrivateLocationOptions): string | null {
  const requested = resolve(path);
  const directory = dirname(requested);
  if (workspace !== undefined) checkStoreLocation(requested, workspace, label);
  if (isMissing(directory)) {
    checkAncestors(existingAncestor(directory), requested, label);
    if (!create) return null;
    mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
  checkPrivate(lstatSync(directory), directory, "directory", label);
  const canonicalDir = realpathSync(directory);
  checkPrivate(lstatSync(canonicalDir), canonicalDir, "directory", label);
  const file = join(canonicalDir, basename(requested));
  if (workspace !== undefined) checkStoreLocation(file, workspace, label);
  checkAncestors(canonicalDir, file, label);
  return file;
}

export interface PrivateDatabaseOptions {
  label?: string;
  /** How long a statement waits for another connection's lock, in milliseconds. */
  timeout: number;
  /** The real path of the served workspace, which must not contain the file. */
  workspace?: string;
}

/**
 * Opens the SQLite file at `path`, creating it and its directory if they are
 * missing. `privateLocation` checks the directory and resolves it to its real
 * path, so the name SQLite opens cannot be redirected by another user. The file is opened first without following a symlink and checked to
 * be a private file of this user, and that descriptor stays open until SQLite
 * has opened the same name and the name is confirmed to still be that file.
 * It is closed before the caller runs any statement, because closing any
 * descriptor of a file drops this process's POSIX locks on it.
 */
export function openPrivateDatabase(path: string, { label = storeLabel, timeout, workspace }: PrivateDatabaseOptions): DatabaseSync {
  const file = privateLocation(path, { create: true, label, workspace });
  let descriptor: number;
  try {
    descriptor = openSync(file, constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ELOOP") {
      throw new Error(`The ${label} ${file} must be a file, not a symlink or other file.`, { cause: error });
    }
    throw error;
  }
  try {
    const verified = fstatSync(descriptor);
    checkPrivate(verified, file, "file", label);
    const database = new DatabaseSync(file, { timeout });
    try {
      const named = lstatSync(file);
      if (named.dev !== verified.dev || named.ino !== verified.ino) {
        throw new Error(`The ${label} ${file} was replaced while it was being opened. Try again.`);
      }
    } catch (error) {
      database.close();
      throw error;
    }
    return database;
  } finally {
    closeSync(descriptor);
  }
}

/**
 * Runs `step` while holding an exclusive lock on `<store>.lock`. The lock is
 * an SQLite exclusive transaction, which holds a POSIX record lock on the
 * file: the kernel releases it when the holder exits or crashes, and no
 * gateway ever deletes or takes over a lock another one holds. A paused
 * holder makes the others fail after `timeout` instead.
 */
function withLock<T>(path: string, timeout: number, step: () => T): T {
  const lock = `${path}.lock`;
  // Created once and kept: a lock file that is deleted and created again
  // could let two gateways each lock a different file.
  const database = openPrivateDatabase(lock, { timeout });
  try {
    try {
      database.exec("BEGIN EXCLUSIVE");
    } catch (error) {
      if ((error as { errcode?: number }).errcode === sqliteBusy) {
        throw new Error(`The paired device store is busy: another gateway held ${lock} for over ${timeout} ms. Try again.`, { cause: error });
      }
      throw error;
    }
    try {
      return step();
    } finally {
      database.exec("ROLLBACK");
    }
  } finally {
    database.close();
  }
}

/** Reads the latest records and writes the changed ones under the lock. */
function updateFile(path: string, change: (devices: DeviceRecord[]) => DeviceRecord[] | null, timeout: number): StoreSnapshot {
  return withLock(path, timeout, () => {
    const current = readFile(path);
    const next = change(current.devices);
    return next ? writeStore(path, next) : current;
  });
}

export interface StoreOptions {
  /** The real path of the served workspace, which must not contain the store. */
  workspace?: string;
  /** How long an update waits for another gateway's update, in milliseconds. */
  lockTimeout?: number;
}

/**
 * The paired device store at `path`. Every read, version check, and update
 * first runs `privateLocation` again, the same checks as the lock and the
 * generation counter get, and uses the file in the real directory it returns.
 * A directory that is moved, replaced, or opened to others after startup
 * stops the gateway from trusting the store even while the file looks
 * unchanged, and a store under a directory another user could replace fails
 * at startup instead of on the first write.
 */
export function openStore(path: string, { workspace, lockTimeout = defaultLockTimeout }: StoreOptions = {}) {
  const missing: StoreSnapshot = { devices: [], version: "missing" };
  const existing = () => privateLocation(path, { create: false, workspace });
  return {
    path,
    read(): StoreSnapshot {
      const file = existing();
      return file ? readFile(file) : missing;
    },
    /** Changes with the file; compare it with a snapshot's version before reusing that snapshot. */
    version() {
      const file = existing();
      return file ? fileVersion(file) : "missing";
    },
    /**
     * Reads the store, lets `change` compute the next records, and writes them
     * while holding the store lock, so gateways sharing the store never lose
     * each other's updates, such as a revocation. `change` returns null to
     * keep the file.
     */
    update(change: (devices: DeviceRecord[]) => DeviceRecord[] | null): StoreSnapshot {
      return updateFile(privateLocation(path, { create: true, workspace }), change, lockTimeout);
    },
  };
}

export type DeviceStore = ReturnType<typeof openStore>;
