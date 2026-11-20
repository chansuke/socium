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

The gateway listens on loopback and serves one workspace. Connected clients share the active conversation and turn. Use the conversation list to start a chat or reopen a saved session. Commands that need approval display the tool details and offer **Allow once** or **Deny**, and **Allow for session** when the request offers it (as the terminal UI does). Warnings that arise during a turn, such as an MCP server that did not start or a renewed token that could not be saved, appear among the turn's notices. You can stop a running turn from any connected client. Close the TUI for this workspace before starting the gateway so the gateway can acquire the workspace lock.

## Connect Android and iOS

Start a private HTTPS reverse proxy to the gateway that your device can reach. Append `--mobile-url https://your-socium-host` to the gateway command above, using the proxy's HTTPS origin.

With Tailscale, enable **HTTPS Certificates** in your tailnet's DNS settings and forward HTTPS traffic to the local gateway:

```sh
sudo tailscale set --operator="$USER"
tailscale serve --bg 4173
```

Then pass `--mobile-url https://<host>.<tailnet>.ts.net` to `npm run start:clients`.

1. Sign in to Socium in your browser with the host access token. Select **Pair a phone**, then **Generate QR code**.
2. Open Socium on the phone, select **Scan QR code**, scan the browser's QR code, and confirm the host address.

Each QR code can be used once within five minutes. A new code replaces the previous unused code. The mobile app saves the paired device token in the operating system's secure storage and removes it when you select **Disconnect**. The gateway issues a new device token when you pair and keeps only its SHA-256 hash in `$XDG_STATE_HOME/socium/paired-devices.json` (`~/.local/state/socium/paired-devices.json` by default; pass `--pairing-store` to use another file), so paired devices stay connected across gateway restarts. With the host access token, `GET /api/devices` lists paired devices and `DELETE /api/devices/<id>` revokes one at once. Use **Enter manually** to connect with the host access token when you cannot scan a QR code.

For local native development, keep the Android SDK in a persistent directory such as `~/Android/Sdk` (`ANDROID_HOME="$HOME/Android/Sdk"`, `ANDROID_AVD_HOME="$HOME/.android/avd"`) or configure Xcode, then run:

```sh
npm run android -w @socium/mobile
npm run ios -w @socium/mobile
```

To run the connection smoke test locally on a running Android KVM emulator with Maestro installed:

```sh
bash scripts/android-emulator-smoke.sh
```

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

The apps share the client protocol and subscription state in `packages/client`. The Node gateway starts the Rust bridge in the chosen workspace. The bridge uses the same `AgentService`, session persistence, and approval checks as the terminal UI.

T3 Code's [shared client runtime](https://github.com/pingdotgg/t3code/tree/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/packages/client-runtime/src), [mobile connection commands](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/mobile/src/connection/onboarding.ts), and [Web subscriptions](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/web/src/hooks/useMediaQuery.ts) informed the split between platform UIs, transport, and shared state.
