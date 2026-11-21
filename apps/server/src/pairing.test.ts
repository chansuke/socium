import { strict as assert } from "node:assert";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { readPairingUrl } from "@socium/client";
import { defaultPairingStore } from "./deviceStore.ts";
import { createPairing } from "./pairing.ts";

function temporaryDirectory(context: TestContext) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "socium-pairing-")));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

const address = "https://socium.test/";

function pair(pairing: ReturnType<typeof createPairing>) {
  const token = pairing.redeem(readPairingUrl(pairing.issue()).code);
  assert(token);
  return `Bearer ${token}`;
}

test("a relative XDG_STATE_HOME is ignored", (context) => {
  const saved = process.env.XDG_STATE_HOME;
  context.after(() => {
    if (saved === undefined) delete process.env.XDG_STATE_HOME;
    else process.env.XDG_STATE_HOME = saved;
  });
  process.env.XDG_STATE_HOME = "relative/state";
  assert(defaultPairingStore().endsWith(join(".local", "state", "socium", "paired-devices.json")));
  assert(!defaultPairingStore().includes("relative"));
  process.env.XDG_STATE_HOME = "/var/state-home";
  assert.equal(defaultPairingStore(), "/var/state-home/socium/paired-devices.json");
});

test("a store in the workspace, or reached through a symlink into it, is refused", (context) => {
  const root = temporaryDirectory(context);
  const workspace = join(root, "project");
  mkdirSync(workspace, { mode: 0o700 });
  symlinkSync(workspace, join(root, "link"));
  for (const storePath of [
    join(workspace, "paired-devices.json"),
    join(workspace, "state", "socium", "paired-devices.json"),
    join(root, "link", "paired-devices.json"),
    workspace,
  ]) {
    assert.throws(() => createPairing(address, { storePath, workspace }), /outside the workspace/, storePath);
  }
  assert(!existsSync(join(workspace, "state")));
  const outside = join(root, "project-state", "paired-devices.json");
  createPairing(address, { storePath: outside, workspace });
});

test("a store that others can reach, or that is a symlink, is refused", (context) => {
  const root = temporaryDirectory(context);
  const cases: [string, (directory: string, store: string) => void, RegExp][] = [
    ["group-readable file", (_directory, store) => writeFileSync(store, "{\"devices\":[]}", { mode: 0o640 }), /mode 600/],
    ["group-accessible directory", (directory) => chmodSync(directory, 0o750), /mode 700/],
    ["symlinked file", (directory, store) => {
      writeFileSync(join(directory, "elsewhere.json"), "{\"devices\":[]}", { mode: 0o600 });
      symlinkSync(join(directory, "elsewhere.json"), store);
    }, /symlink/],
  ];
  for (const [name, prepare, error] of cases) {
    const directory = join(root, name.replaceAll(" ", "-"));
    mkdirSync(directory, { mode: 0o700 });
    const storePath = join(directory, "paired-devices.json");
    prepare(directory, storePath);
    assert.throws(() => createPairing(address, { storePath }), error, name);
  }

  const target = join(root, "target");
  mkdirSync(target, { mode: 0o700 });
  symlinkSync(target, join(root, "linked"));
  assert.throws(() => createPairing(address, { storePath: join(root, "linked", "paired-devices.json") }), /symlink/);

  // A store that turns unsafe while the gateway runs stops authorizing.
  const storePath = join(root, "fine", "paired-devices.json");
  const pairing = createPairing(address, { storePath });
  const device = pair(pairing);
  chmodSync(storePath, 0o644);
  assert.throws(() => pairing.authorizes(device), /mode 600/);
});
