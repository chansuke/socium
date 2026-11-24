import { strict as assert } from "node:assert";
import { test } from "node:test";
import { createHostStorage, createPairingStorage } from "./hostStorage.ts";
import type { StorageProblem } from "./hostStorage.ts";
import type { PairedHost } from "./pairedHost.ts";

const host: PairedHost = { origin: "https://socium.test", token: "t".repeat(43) };

test("a failed delete is reported and can be tried again", async () => {
  const saved: (PairedHost | null)[] = [];
  let failures = 1;
  const store = createHostStorage(async (value) => {
    if (value === null && failures-- > 0) throw new Error("The keychain is locked.");
    saved.push(value);
  });
  await store(host);
  await assert.rejects(store(null), /keychain is locked/);
  assert.deepEqual(saved, [host], "the token looked forgotten though it was kept");
  await store(null);
  assert.deepEqual(saved, [host, null]);
});

test("saves run in order, and a failure does not stop the ones after it", async () => {
  const order: string[] = [];
  const store = createHostStorage(async (value) => {
    await new Promise((resolve) => setTimeout(resolve, value ? 10 : 0));
    if (value?.origin === "https://fails.test") throw new Error("Full.");
    order.push(value?.origin ?? "deleted");
  });
  const results = await Promise.allSettled([
    store(host),
    store({ ...host, origin: "https://fails.test" }),
    store(null),
  ]);
  assert.deepEqual(results.map(({ status }) => status), ["fulfilled", "rejected", "fulfilled"]);
  assert.deepEqual(order, ["https://socium.test", "deleted"]);
});

test("a save that fails while a forget is running leaves no error and no way to save the forgotten token again", async () => {
  const saved: (PairedHost | null)[] = [];
  let failSave!: () => void;
  const problems: (StorageProblem | null)[] = [];
  const storage = createPairingStorage(async (value) => {
    if (value) {
      await new Promise<void>((_resolve, reject) => failSave = () => reject(new Error("The keychain is locked.")));
    }
    saved.push(value);
  }, (problem) => problems.push(problem));
  let forgotten = 0;

  storage.save(host);
  const forgetting = storage.forget(() => forgotten++);
  await until(() => failSave !== undefined);
  failSave();
  await forgetting;
  assert.equal(forgotten, 1);
  assert.deepEqual(saved, [null]);
  assert.equal(problems.at(-1), null);
  assert(problems.every((problem) => problem === null), "a stale save error was shown");
});

test("a save error offers to try again until a forget starts", async () => {
  const saved: (PairedHost | null)[] = [];
  let fail = true;
  const problems: (StorageProblem | null)[] = [];
  const storage = createPairingStorage(async (value) => {
    if (value && fail) throw new Error("Full.");
    saved.push(value);
  }, (problem) => problems.push(problem));
  storage.save(host);
  await until(() => problems.at(-1) != null);
  fail = false;
  problems.at(-1)?.retry();
  await until(() => saved.length === 1);
  assert.deepEqual(saved, [host]);
  assert.equal(problems.at(-1), null);

  fail = true;
  storage.save(host);
  await until(() => problems.at(-1) != null);
  await storage.forget(() => {});
  assert.equal(problems.at(-1), null, "the save error outlived the forget");
  assert.deepEqual(saved, [host, null]);
});

test("a failed forget offers to try again and finishes only once it succeeds", async () => {
  let failures = 1;
  const problems: (StorageProblem | null)[] = [];
  const storage = createPairingStorage(async (value) => {
    if (value === null && failures-- > 0) throw new Error("The keychain is locked.");
  }, (problem) => problems.push(problem));
  let forgotten = 0;
  await storage.forget(() => forgotten++);
  assert.equal(forgotten, 0);
  assert.match(problems.at(-1)?.message ?? "", /Could not remove/);
  problems.at(-1)?.retry();
  await until(() => forgotten === 1);
  assert.equal(problems.at(-1), null);
});

async function until(condition: () => boolean) {
  for (let tries = 0; !condition(); tries++) {
    assert(tries < 200, "timed out");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
