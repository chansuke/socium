import { dirname, join } from "node:path";
import { checkDirectory, checkStoreLocation, defaultPairingStore, openPrivateDatabase } from "./deviceStore.ts";

const label = "gateway generation counter";

/** Where gateways on this machine count their starts, beside the default pairing store. */
export function defaultGenerationStore() {
  return join(dirname(defaultPairingStore()), "gateway-generation.sqlite");
}

export interface GenerationOptions {
  /** The real path of the served workspace, which must not contain the counter. */
  workspace?: string;
}

/**
 * Takes the next gateway generation from the counter at `path`: each call,
 * from any gateway, gets a number larger than every earlier one, even across
 * crashes, because SQLite increments and commits it in one statement.
 *
 * The counter gets the same checks as the paired device store before SQLite
 * opens it: outside the workspace, a directory with mode 700, and a file with
 * mode 600, both of this user and neither a symlink, under directories that
 * no other user can change. SQLite keeps its rollback
 * journal beside the file only during a write and gives it the file's mode.
 */
export function nextGeneration(path = defaultGenerationStore(), { workspace }: GenerationOptions = {}): number {
  if (workspace !== undefined) checkStoreLocation(path, workspace, label);
  checkDirectory(path, true, label);
  const database = openPrivateDatabase(path, { label, timeout: 5000, workspace });
  try {
    // No write-ahead log, so no -wal or -shm files stay beside the counter.
    database.exec("PRAGMA journal_mode = DELETE");
    database.exec("CREATE TABLE IF NOT EXISTS generation (id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL)");
    const row = database
      .prepare("INSERT INTO generation (id, value) VALUES (1, 1) ON CONFLICT (id) DO UPDATE SET value = value + 1 RETURNING value")
      .get() as { value: number };
    return row.value;
  } finally {
    database.close();
  }
}
