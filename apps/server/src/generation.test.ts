import { strict as assert } from "node:assert";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { TestContext } from "node:test";
import { othersCanReplace } from "./deviceStore.ts";
import { defaultGenerationStore, nextGeneration } from "./generation.ts";

function temporaryDirectory(context: TestContext) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "socium-generation-")));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test("each gateway start takes a larger generation, kept across restarts in a private file", (context) => {
  const directory = temporaryDirectory(context);
  const path = join(directory, "state", "gateway-generation.sqlite");
  assert.deepEqual([nextGeneration(path), nextGeneration(path), nextGeneration(path)], [1, 2, 3]);
  assert.equal(statSync(path).mode & 0o777, 0o600);
  assert.equal(statSync(join(directory, "state")).mode & 0o777, 0o700);
  // No journal, write-ahead log, or shared memory file stays beside it.
  assert.deepEqual(readdirSync(join(directory, "state")), ["gateway-generation.sqlite"]);
});

test("the default counter sits beside the default pairing store, and a relative XDG_STATE_HOME is ignored", (context) => {
  const saved = process.env.XDG_STATE_HOME;
  context.after(() => {
    if (saved === undefined) delete process.env.XDG_STATE_HOME;
    else process.env.XDG_STATE_HOME = saved;
  });
  process.env.XDG_STATE_HOME = "relative/state";
  assert(defaultGenerationStore().endsWith(join(".local", "state", "socium", "gateway-generation.sqlite")));
  assert(!defaultGenerationStore().includes("relative"));
  process.env.XDG_STATE_HOME = "/var/state-home";
  assert.equal(defaultGenerationStore(), "/var/state-home/socium/gateway-generation.sqlite");
});

test("a counter in the workspace, or reached through a symlink into it, is refused before it is created", (context) => {
  const root = temporaryDirectory(context);
  const workspace = join(root, "project");
  mkdirSync(workspace, { mode: 0o700 });
  symlinkSync(workspace, join(root, "link"));
  for (const path of [
    join(workspace, "gateway-generation.sqlite"),
    join(workspace, "state", "gateway-generation.sqlite"),
    join(root, "link", "gateway-generation.sqlite"),
    // A name that only starts with two dots is still inside.
    join(workspace, "..hidden", "gateway-generation.sqlite"),
  ]) {
    assert.throws(() => nextGeneration(path, { workspace }), /Keep the gateway generation counter outside the workspace/, path);
  }
  assert.deepEqual(readdirSync(workspace), []);
  assert.equal(nextGeneration(join(root, "project..state", "gateway-generation.sqlite"), { workspace }), 1);
});

test("missing counter directories under a symlink into the workspace are refused before any is made", (context) => {
  const root = temporaryDirectory(context);
  const workspace = join(root, "project");
  mkdirSync(join(workspace, "data"), { recursive: true, mode: 0o700 });
  symlinkSync(join(workspace, "data"), join(root, "state"));
  const path = join(root, "state", "socium", "counter", "gateway-generation.sqlite");
  assert.throws(() => nextGeneration(path, { workspace }), /Keep the gateway generation counter outside the workspace/);
  assert.deepEqual(readdirSync(join(workspace, "data")), []);
});

test("a counter that others can reach, or that is a symlink, is refused", (context) => {
  const root = temporaryDirectory(context);
  const cases: [string, (directory: string, path: string) => void, RegExp][] = [
    ["world-readable file", (_directory, path) => {
      nextGeneration(path);
      chmodSync(path, 0o644);
    }, /gateway generation counter .* must have mode 600/],
    ["world-readable directory", (directory) => chmodSync(directory, 0o755), /gateway generation counter .* must have mode 700/],
    ["symlinked file", (directory, path) => {
      writeFileSync(join(directory, "elsewhere.sqlite"), "", { mode: 0o600 });
      symlinkSync(join(directory, "elsewhere.sqlite"), path);
    }, /gateway generation counter .* must be a file, not a symlink/],
  ];
  for (const [name, prepare, error] of cases) {
    const directory = join(root, name.replaceAll(" ", "-"));
    mkdirSync(directory, { mode: 0o700 });
    const path = join(directory, "gateway-generation.sqlite");
    prepare(directory, path);
    assert.throws(() => nextGeneration(path), error, name);
  }
  // The symlink target was never opened as a database.
  assert.equal(statSync(join(root, "symlinked-file", "elsewhere.sqlite")).size, 0);

  const target = join(root, "target");
  mkdirSync(target, { mode: 0o700 });
  symlinkSync(target, join(root, "linked"));
  assert.throws(() => nextGeneration(join(root, "linked", "gateway-generation.sqlite")), /must be a directory, not a symlink/);
  assert(!existsSync(join(target, "gateway-generation.sqlite")));
});

test("a counter under a directory that another user could replace is refused, also when reached through a symlink", (context) => {
  const root = temporaryDirectory(context);
  const refused = /gateway generation counter .* is under .*, which another user could replace/;
  const cases: [string, number][] = [
    ["world-writable", 0o777],
    ["group-writable", 0o775],
  ];
  for (const [name, mode] of cases) {
    const ancestor = join(root, name.replaceAll(" ", "-"));
    mkdirSync(join(ancestor, "state"), { recursive: true, mode: 0o700 });
    chmodSync(ancestor, mode);
    const path = join(ancestor, "state", "gateway-generation.sqlite");
    assert.throws(() => nextGeneration(path), refused, name);
    assert(!existsSync(path), name);
  }

  // Sticky only protects entries when root owns the directory, and the test
  // owns this one, which is root only when the tests run as root.
  const sticky = join(root, "sticky");
  mkdirSync(join(sticky, "state"), { recursive: true, mode: 0o700 });
  chmodSync(sticky, 0o1777);
  const underSticky = join(sticky, "state", "gateway-generation.sqlite");
  if (process.getuid?.() === 0) assert.equal(nextGeneration(underSticky), 1);
  else assert.throws(() => nextGeneration(underSticky), refused);

  // A symlink to a safe-looking path does not hide the shared directory it leads through.
  const shared = join(root, "shared");
  mkdirSync(join(shared, "state"), { recursive: true, mode: 0o700 });
  chmodSync(shared, 0o777);
  symlinkSync(shared, join(root, "alias"));
  assert.throws(() => nextGeneration(join(root, "alias", "state", "gateway-generation.sqlite")), refused);
  assert(!existsSync(join(shared, "state", "gateway-generation.sqlite")));

  // Directories only this user can write, readable by others or not, are fine,
  // and a symlinked ancestor opens the counter at its real location.
  const guarded = join(root, "guarded");
  mkdirSync(join(guarded, "state"), { recursive: true, mode: 0o700 });
  chmodSync(guarded, 0o755);
  symlinkSync(guarded, join(root, "guarded-alias"));
  assert.equal(nextGeneration(join(guarded, "state", "gateway-generation.sqlite")), 1);
  assert.equal(nextGeneration(join(root, "guarded-alias", "state", "gateway-generation.sqlite")), 2);
});

test("a sticky world-writable ancestor is allowed only when root owns it, whoever runs the gateway", () => {
  const directory = (uid: number, mode: number) => ({ uid, mode: 0o40000 | mode, isDirectory: () => true });
  for (const gateway of [0, 1000]) {
    // Such as /tmp.
    assert(!othersCanReplace(directory(0, 0o1777), gateway), `root's sticky directory, gateway uid ${gateway}`);
    assert(othersCanReplace(directory(1001, 0o1777), gateway), `another user's sticky directory, gateway uid ${gateway}`);
    assert(othersCanReplace(directory(0, 0o777), gateway), `root's world-writable directory, gateway uid ${gateway}`);
    assert(!othersCanReplace(directory(gateway, 0o755), gateway), `own directory, gateway uid ${gateway}`);
  }
  assert(othersCanReplace(directory(1000, 0o1777), 1000), "own sticky world-writable directory");
  assert(othersCanReplace(directory(1001, 0o755), 1000), "another user's directory");
  assert(othersCanReplace({ uid: 1000, mode: 0o120777, isDirectory: () => false }, 1000), "a symlink");
});

test("a counter under a refused ancestor creates none of the missing directories", (context) => {
  const ancestor = join(temporaryDirectory(context), "shared");
  mkdirSync(ancestor, { mode: 0o700 });
  chmodSync(ancestor, 0o775);
  const path = join(ancestor, "missing", "state", "gateway-generation.sqlite");
  assert.throws(() => nextGeneration(path), /gateway generation counter .* is under .*shared, which another user could replace/);
  assert.deepEqual(readdirSync(ancestor), []);
});
