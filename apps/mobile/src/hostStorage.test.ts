import { strict as assert } from "node:assert";
import { test } from "node:test";
import { createHostStorage } from "./hostStorage.ts";
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
