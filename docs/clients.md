# Web and mobile clients

Run the Socium agent and gateway on a Linux host with [the same prerequisites as the terminal client](linux-sandbox.md#installation-and-support). Install Node.js 24 or newer for the gateway and client builds.

## Run the gateway and web client

From the Socium checkout:

```sh
npm ci
npm run build:web
nix develop --command cargo build --locked -p socium-tui
SOCIUM_BIN="$PWD/target/debug/socium" nix develop --command npm run start:clients -- --workspace /absolute/path/to/your/project
```

Open `http://127.0.0.1:4173` and enter the access token printed by the gateway. If you already have a Socium binary built from this checkout on your PATH, omit `SOCIUM_BIN`. Configure the provider and its key on the Linux host as you would for the terminal client. For Vite development, run `npm run dev:web` with the gateway running. If you set a custom gateway port, set `SOCIUM_GATEWAY_PORT` to the same port for Vite.

The gateway listens on loopback and serves one workspace. Connected clients share the active conversation and turn. Use the conversation list to start a chat or reopen a saved session. Commands that need approval display the tool details and offer **Allow once** or **Deny**. When the request supports it, **Allow for session** also allows what the request names, such as `cargo test` commands, until Socium exits. Warnings that arise during a turn, such as an MCP server that did not start or a renewed token that could not be saved, appear among the turn's notices. You can stop a running turn from any connected client. Close the TUI for this workspace before starting the gateway so the gateway can acquire the workspace lock.

The model switcher in the top bar (in **Conversations** on mobile) lists the host's models. Switch between turns; a model the host cannot use now, such as one without a key, is listed with the reason and cannot be selected. While a turn runs, a status line shows what the agent is doing, such as waiting for the provider or running a program. It names only the program, never its arguments. When the agent keeps a plan, a panel above the transcript shows its goal, its progress, and each task.

Assistant replies are shown as Markdown on both clients. Links open only for absolute `http` and `https` URLs; other links show as their text. Tool calls show as their first line and open to the full output. On mobile, a reply is shown up to its first 50,000 characters and then ends with `…`.

The web client receives updates over Server-Sent Events. While the stream reconnects, the client says so and holds the model switcher and commands until it is back. The mobile app polls for updates, because React Native cannot read a streamed response. If the host rejects the token, such as after a revocation, the client stops connecting and asks you to pair again. Each gateway start takes a larger generation number from `$XDG_STATE_HOME/socium/gateway-generation.sqlite` (`~/.local/state/socium/gateway-generation.sqlite` by default), so after a restart the clients ignore late replies and events from the gateway it replaced. Every gateway that the same user runs on the machine shares this counter, even with `--pairing-store`. The gateway checks it like the pairing store below: it refuses the counter when it is inside the workspace, is a symlink, belongs to another user, or has a directory mode other than 700 or a file mode other than 600.

## Connect Android and iOS

Start a private HTTPS reverse proxy to the gateway that your device can reach. Append `--mobile-url https://your-socium-host` to the gateway command above, using the proxy's HTTPS origin.

With Tailscale, enable **HTTPS Certificates** in your tailnet's DNS settings and forward HTTPS traffic to the local gateway:

```sh
sudo tailscale set --operator="$USER"
tailscale serve --bg 4173
```

Then pass `--mobile-url https://<host>.<tailnet>.ts.net` to `npm run start:clients`.

If another proxy, such as Traefik, already serves port 443 on the host, serve the gateway on another HTTPS port and include that port in the mobile URL:

```sh
tailscale serve --bg --https=8443 4173
```

Then pass `--mobile-url https://<host>.<tailnet>.ts.net:8443`. See the [`tailscale serve` documentation](https://tailscale.com/kb/1242/tailscale-serve).

1. Sign in to Socium in your browser with the host access token. Select **Pair a phone**, then **Generate QR code**.
2. Open Socium on the phone, select **Scan QR code**, scan the browser's QR code, and confirm the host address.

Each QR code can be used once within five minutes. A new code replaces the previous unused code. The mobile app saves the paired device token in the operating system's secure storage and removes it when you select **Disconnect** or **Pair again**. If the phone cannot remove it, the app says so and offers **Try again**.

The gateway issues a new device token when you pair and keeps only its SHA-256 hash in `$XDG_STATE_HOME/socium/paired-devices.json` (`~/.local/state/socium/paired-devices.json` by default; pass `--pairing-store` to use another file), so paired devices stay connected across gateway restarts. The store must stay outside the workspace, where the agent's tools could read or change it. The gateway creates its directory with mode 700 and the file with mode 600. It refuses the store when it is inside the workspace, is a symlink, belongs to another user, or has a wider mode, and checks this again on every use, so a store changed while the gateway runs stops authorizing devices. Gateways that share a store take turns with an exclusive lock on `paired-devices.json.lock`, which the operating system releases if a gateway exits.

The **Pair a phone** dialog lists paired devices with when each was paired and last used. Select **Revoke** to remove a device: its token stops working at once, and its open event streams (SSE) close at once, including those on other gateways that share the store. A gateway that cannot watch the store directory, such as when the system has no file watches left, closes the streams of a device revoked on another gateway at the next keepalive instead, within 15 seconds. Only the host access token can list and revoke devices, also through `GET /api/devices` and `DELETE /api/devices/<id>`. Use **Enter manually** to connect with the host access token when you cannot scan a QR code.

For local native development, keep the Android SDK in a persistent directory such as `~/Android/Sdk` (`ANDROID_HOME="$HOME/Android/Sdk"`, `ANDROID_AVD_HOME="$HOME/.android/avd"`) or configure Xcode, then run:

```sh
npm run android -w @socium/mobile
npm run ios -w @socium/mobile
```

To run the connection smoke test locally on a running Android KVM emulator with Maestro installed:

```sh
bash scripts/android-emulator-smoke.sh
```

`apps/mobile/.maestro/validation.yaml` also checks that the manual form refuses an `http` host address, that **Back** clears the error, and that **Scan QR code** opens the scanner. Run it with `maestro test apps/mobile/.maestro/validation.yaml` after the app is installed.

## Run a smoke test on Expo Simulators

The Expo project ID is in [the app configuration](../apps/mobile/app.json). From `apps/mobile`, log in to the Expo account that has access to the project and check simulator availability.

```sh
npx --yes eas-cli@27.11.0 whoami
npx --yes eas-cli@27.11.0 simulator:availability --json
```

Build an Android APK and an iOS Simulator app with the `simulator` profile. The first Android build may prompt to set up Android signing credentials.

```sh
npx --yes eas-cli@27.11.0 build --platform android --profile simulator
npx --yes eas-cli@27.11.0 build --platform ios --profile simulator
```

Provide the two completed build IDs to the **Expo Simulator smoke** GitHub Actions workflow. Configure `EXPO_TOKEN` as a repository secret for an Expo account with project and Simulators access. The workflow opens each app, checks the connection screen, captures a screenshot, and stops its simulator session. To run the same smoke test locally, install `jq` and run these commands from `apps/mobile`:

```sh
bash ../../scripts/expo-simulator-smoke.sh android ANDROID_BUILD_ID
bash ../../scripts/expo-simulator-smoke.sh ios IOS_BUILD_ID
```

To check the full mobile flow interactively, connect the app to a test workspace through a reachable HTTPS host. Send a prompt, check that the reply arrives, review a tool approval, and verify that a saved conversation can be reopened. [Expo Simulators documentation](https://docs.expo.dev/eas/simulators/get-started/).

## Build for a physical iPhone

From `apps/mobile`, register the test iPhone with `npx --yes eas-cli@27.11.0 device:create`. Sign in with your Apple Developer account, choose **Website**, and open the registration URL in Safari on that iPhone.

Build and sign the app with the `ios-device` profile:

```sh
npx --yes eas-cli@27.11.0 build --platform ios --profile ios-device
```

Include the test iPhone when EAS prompts for devices. After the build finishes, open the EAS build page on the iPhone and tap **Install**. This profile bundles JavaScript so the app can launch without Metro. [Expo internal distribution documentation](https://docs.expo.dev/build/internal-distribution/).

## Architecture

The apps share the client protocol and subscription state in `packages/client`. Clients receive workspace updates as Server-Sent Events from `GET /api/events` and poll `GET /api/state` instead where the platform cannot read a streamed response, such as React Native, or where a proxy buffers the stream. The Node gateway starts the Rust bridge in the chosen workspace. The bridge uses the same `AgentService`, session persistence, and approval checks as the terminal UI.

T3 Code's [shared client runtime](https://github.com/pingdotgg/t3code/tree/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/packages/client-runtime/src), [mobile connection commands](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/mobile/src/connection/onboarding.ts), and [Web subscriptions](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/web/src/hooks/useMediaQuery.ts) informed the split between platform UIs, transport, and shared state.
