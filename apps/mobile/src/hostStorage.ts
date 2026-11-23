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
