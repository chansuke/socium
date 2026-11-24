import type { PairedHost } from "./pairedHost";

/**
 * Saves or forgets the paired host one call at a time, in order. Each call
 * settles with its own outcome, so a failed delete is reported instead of
 * leaving a revoked token on the phone unnoticed, and a failure does not
 * hold up the calls after it.
 */
export function createHostStorage(save: (host: PairedHost | null) => Promise<void>) {
  let queue: Promise<unknown> = Promise.resolve();
  return (host: PairedHost | null) => {
    const next = queue.then(() => save(host));
    queue = next.catch(() => {});
    return next;
  };
}

/** A pairing this phone failed to save or forget, with the way to try again. */
export interface StorageProblem {
  message: string;
  retry: () => void;
}

/**
 * Saves and forgets the paired host for the app, and reports the failure of
 * the latest save or forget with a way to try again, or null once a new one
 * starts. Only the latest one may report or finish: a save that fails after a
 * forget began must not offer to save the forgotten token again.
 */
export function createPairingStorage(save: (host: PairedHost | null) => Promise<void>, report: (problem: StorageProblem | null) => void) {
  const store = createHostStorage(save);
  /** Counts saves and forgets; each knows its own number. */
  let latest = 0;

  function saveHost(host: PairedHost) {
    const operation = ++latest;
    report(null);
    store(host).catch(() => {
      if (operation !== latest) return;
      report({
        message: "Paired, but this phone could not save the pairing, so it will ask again after a restart.",
        retry: () => saveHost(host),
      });
    });
  }

  /** Forgets the saved token, then calls `forgotten`; stays put if the phone could not forget it. */
  async function forget(forgotten: () => void) {
    const operation = ++latest;
    report(null);
    try {
      await store(null);
    } catch {
      if (operation === latest) {
        report({ message: "Could not remove the saved pairing from this phone. Try again.", retry: () => void forget(forgotten) });
      }
      return;
    }
    if (operation === latest) forgotten();
  }

  return { save: saveHost, forget };
}
