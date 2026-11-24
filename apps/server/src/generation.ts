import { chmodSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { defaultPairingStore } from "./deviceStore.ts";

/** Where gateways on this machine count their starts, beside the default pairing store. */
export function defaultGenerationStore() {
  return join(dirname(defaultPairingStore()), "gateway-generation.sqlite");
}

/**
 * Takes the next gateway generation from the counter at `path`: each call,
 * from any gateway, gets a number larger than every earlier one, even across
 * crashes, because SQLite increments and commits it in one statement.
 */
export function nextGeneration(path = defaultGenerationStore()): number {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const database = new DatabaseSync(path, { timeout: 5000 });
  try {
    chmodSync(path, 0o600);
    database.exec("CREATE TABLE IF NOT EXISTS generation (id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL)");
    const row = database
      .prepare("INSERT INTO generation (id, value) VALUES (1, 1) ON CONFLICT (id) DO UPDATE SET value = value + 1 RETURNING value")
      .get() as { value: number };
    return row.value;
  } finally {
    database.close();
  }
}
