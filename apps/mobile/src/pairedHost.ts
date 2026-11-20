import * as SecureStore from "expo-secure-store";
import { mobileEndpoint } from "@socium/client";

const key = "socium.paired-host";

export interface PairedHost {
  origin: string;
  token: string;
}

export async function loadPairedHost(): Promise<PairedHost | null> {
  try {
    const stored = await SecureStore.getItemAsync(key);
    if (!stored) return null;
    const { origin, token } = JSON.parse(stored);
    if (typeof origin !== "string" || typeof token !== "string" || !/^[\w-]{43}$/.test(token)) return null;
    return { origin: mobileEndpoint(origin), token };
  } catch {
    return null;
  }
}

export async function savePairedHost(host: PairedHost | null) {
  if (host === null) {
    await SecureStore.deleteItemAsync(key);
    return;
  }
  await SecureStore.setItemAsync(key, JSON.stringify(host), {
    keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY,
  });
}
