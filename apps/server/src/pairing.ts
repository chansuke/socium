import { createHash, randomBytes } from "node:crypto";
import { mobileEndpoint } from "@socium/client";

const lifetime = 5 * 60_000;

function hash(value: string) {
  return createHash("sha256").update(value).digest("hex");
}

export function createPairing(address: string, clock = Date.now) {
  const origin = mobileEndpoint(address);
  const pairedDevices = new Set<string>();
  let pending: { hash: string; expires: number } | undefined;

  return {
    issue() {
      const code = randomBytes(32).toString("base64url");
      pending = { hash: hash(code), expires: clock() + lifetime };
      return `${origin}/pair#${code}`;
    },
    redeem(code: string) {
      if (!pending || clock() >= pending.expires || pending.hash !== hash(code)) return null;
      pending = undefined;
      const token = randomBytes(32).toString("base64url");
      pairedDevices.add(hash(`Bearer ${token}`));
      return token;
    },
    authorizes(authorization: string) {
      return pairedDevices.has(hash(authorization));
    },
  };
}

export type Pairing = ReturnType<typeof createPairing>;
