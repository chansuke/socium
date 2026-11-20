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

The gateway listens on loopback and serves one workspace. Connected clients share the active conversation and turn. Use the conversation list to start a chat or reopen a saved session. Commands that need approval display the tool details and offer **Allow once** or **Deny**. You can stop a running turn from any connected client. Close the TUI for this workspace before starting the gateway so the gateway can acquire the workspace lock.

## Connect Android and iOS

Start a private HTTPS reverse proxy to the gateway that your device can reach. Enter the proxy's HTTPS origin and the gateway's access token in the mobile app. The access token grants access to the workspace transcript and its approval controls. The clients keep it in memory until you disconnect or close the app.

For local native development, configure the Android SDK or Xcode, then run:

```sh
npm run android -w @socium/mobile
npm run ios -w @socium/mobile
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

## Architecture

The apps share the client protocol and subscription state in `packages/client`. The Node gateway starts the Rust bridge in the chosen workspace. The bridge uses the same `AgentService`, session persistence, and approval checks as the terminal UI.

T3 Code's [shared client runtime](https://github.com/pingdotgg/t3code/tree/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/packages/client-runtime/src), [mobile connection commands](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/mobile/src/connection/onboarding.ts), and [Web subscriptions](https://github.com/pingdotgg/t3code/blob/4d5bf2d5579f10a521e0ab4899fcef96de5e4b41/apps/web/src/hooks/useMediaQuery.ts) informed the split between platform UIs, transport, and shared state.
