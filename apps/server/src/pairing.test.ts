import { strict as assert } from "node:assert";
import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { pathToFileURL } from "node:url";
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
    // A name that only starts with two dots is still inside.
    join(workspace, "..hidden", "paired-devices.json"),
    join(workspace, "..", "project", "..state", "paired-devices.json"),
  ]) {
    assert.throws(() => createPairing(address, { storePath, workspace }), /outside the workspace/, storePath);
  }
  assert(!existsSync(join(workspace, "state")));
  assert(!existsSync(join(workspace, "..hidden")));
  for (const outside of [join(root, "project-state", "paired-devices.json"), join(root, "project..state", "paired-devices.json")]) {
    pair(createPairing(address, { storePath: outside, workspace }));
  }
});

test("a store directory moved into the workspace after startup stops authorizing", (context) => {
  const root = temporaryDirectory(context);
  const workspace = join(root, "project");
  mkdirSync(workspace, { mode: 0o700 });
  const directory = join(root, "state");
  const pairing = createPairing(address, { storePath: join(directory, "paired-devices.json"), workspace });
  const device = pair(pairing);
  assert(pairing.authorizes(device));
  // The same files, now where the agent's tools can rewrite them.
  cpSync(directory, join(workspace, "state"), { recursive: true });
  renameSync(directory, join(root, "moved"));
  symlinkSync(join(workspace, "state"), directory);
  assert.throws(() => pairing.authorizes(device), /outside the workspace/);
  assert.throws(() => pairing.devices(), /outside the workspace/);
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

test("a store directory opened to others after startup stops authorizing, though the file is unchanged", (context) => {
  const directory = join(temporaryDirectory(context), "state");
  const pairing = createPairing(address, { storePath: join(directory, "paired-devices.json") });
  const device = pair(pairing);
  // The second check reuses the records the first one wrote.
  assert(pairing.authorizes(device));
  assert(pairing.authorizes(device));
  chmodSync(directory, 0o755);
  assert.throws(() => pairing.authorizes(device), /mode 700/);
  assert.throws(() => pairing.devices(), /mode 700/);
  assert.throws(() => pairing.revoke("device"), /mode 700/);
  chmodSync(directory, 0o700);
  assert(pairing.authorizes(device));
});

test("a store that belongs to another user is refused, at startup and later", (context) => {
  const storePath = join(temporaryDirectory(context), "state", "paired-devices.json");
  const pairing = createPairing(address, { storePath });
  const device = pair(pairing);
  assert(pairing.authorizes(device));
  assert(pairing.authorizes(device));
  const uid = process.getuid?.();
  if (uid === undefined) {
    context.skip("This platform has no user ids.");
    return;
  }
  // As if the gateway now ran as another user than the store's owner.
  const getuid = process.getuid;
  context.after(() => process.getuid = getuid);
  process.getuid = () => uid + 1;
  assert.throws(() => createPairing(address, { storePath }), /belong to the user/);
  assert.throws(() => pairing.authorizes(device), /belong to the user/);
});

/** Runs pairing operations in another process, as a second gateway on the same store. */
async function otherGateway(storePath: string, script: string) {
  const pairingModule = pathToFileURL(join(import.meta.dirname, "pairing.ts")).href;
  const clientModule = pathToFileURL(join(import.meta.dirname, "../../../packages/client/src/client.ts")).href;
  const child = spawn(process.execPath, ["--input-type=module", "-e", `
    import { createPairing } from ${JSON.stringify(pairingModule)};
    import { readPairingUrl } from ${JSON.stringify(clientModule)};
    const storePath = ${JSON.stringify(storePath)};
    let minute = 0;
    // Each call is a minute later, so every authorization rewrites lastUsedAt.
    const pairing = createPairing("https://socium.test/", { storePath, clock: () => Date.now() + 60_000 * ++minute });
    const pair = () => pairing.redeem(readPairingUrl(pairing.issue()).code);
    ${script}
  `], { stdio: ["ignore", "inherit", "inherit"] });
  const [code] = await once(child, "exit");
  assert.equal(code, 0);
}

test("gateways sharing a store keep every pairing and never bring back a revoked device", { timeout: 60_000 }, async (context) => {
  const storePath = join(temporaryDirectory(context), "state", "paired-devices.json");
  const host = createPairing(address, { storePath });
  const revoked = pair(host);
  const [device] = host.devices();
  assert(device);

  const rounds = 40;
  await Promise.all([
    // Both pair devices and keep using the doomed one, each from its own cache.
    otherGateway(storePath, `for (let i = 0; i < ${rounds}; i++) { pair(); pairing.authorizes(${JSON.stringify(revoked)}); }`),
    otherGateway(storePath, `for (let i = 0; i < ${rounds}; i++) { pair(); pairing.authorizes(${JSON.stringify(revoked)}); }`),
    // Revokes it while the others are halfway through.
    otherGateway(storePath, `
      while (pairing.devices().length < ${rounds}) await new Promise((resolve) => setTimeout(resolve, 1));
      if (!pairing.revoke(${JSON.stringify(device.id)})) process.exit(2);
    `),
  ]);

  const stored = JSON.parse(readFileSync(storePath, "utf8")).devices as { id: string }[];
  assert.equal(stored.length, rounds * 2, "a pairing was lost");
  assert(!stored.some(({ id }) => id === device.id), "the revoked device came back");
  assert.equal(host.authorizes(revoked), false);
  assert(!existsSync(`${storePath}.lock`));
});

test("a lock left by a stopped gateway is taken over", (context) => {
  const directory = temporaryDirectory(context);
  const storePath = join(directory, "paired-devices.json");
  // No process has this pid: Linux caps pids well below it.
  writeFileSync(`${storePath}.lock`, "2147483646 stale\n", { mode: 0o600 });
  const pairing = createPairing(address, { storePath });
  pair(pairing);
  assert.equal(pairing.devices().length, 1);
  assert(!existsSync(`${storePath}.lock`));
});
