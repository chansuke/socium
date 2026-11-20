import { randomBytes } from "node:crypto";
import { realpathSync } from "node:fs";
import { resolve } from "node:path";
import { parseArgs } from "node:util";
import { createBridge } from "./bridge.ts";
import { createHttpServer } from "./http.ts";
import { createPairing } from "./pairing.ts";

const { values } = parseArgs({
  options: {
    workspace: { type: "string" },
    port: { type: "string", default: "4173" },
    "mobile-url": { type: "string" },
    "pairing-store": { type: "string" },
  },
});
if (!values.workspace) {
  throw new Error("Usage: npm run start:clients -- --workspace /path/to/project [--port 4173] [--mobile-url https://host] [--pairing-store /path/to/paired-devices.json]");
}
const port = Number(values.port);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("Enter a valid TCP port.");
const configuredToken = process.env.SOCIUM_CLIENT_TOKEN;
const token = configuredToken ?? randomBytes(32).toString("base64url");
if (!/^[a-zA-Z0-9_-]{32,}$/.test(token)) {
  throw new Error("SOCIUM_CLIENT_TOKEN must contain at least 32 base64url characters.");
}
const storePath = values["pairing-store"] ? resolve(values["pairing-store"]) : undefined;
const pairing = values["mobile-url"] ? createPairing(values["mobile-url"], { storePath }) : undefined;
const workspace = realpathSync(values.workspace);
const bridge = createBridge(process.env.SOCIUM_BIN ?? "socium", workspace);
const server = createHttpServer(bridge, token, resolve(import.meta.dirname, "../../web/dist"), pairing);

server.on("error", (error) => {
  bridge.close();
  console.error(error.message);
  process.exitCode = 1;
});
server.listen(port, "127.0.0.1", () => {
  console.log(`Socium • http://127.0.0.1:${port} • ${workspace}`);
  if (!configuredToken) console.log(`Access token: ${token}`);
});

function close() {
  server.close();
  bridge.close();
}

process.once("SIGINT", close);
process.once("SIGTERM", close);
