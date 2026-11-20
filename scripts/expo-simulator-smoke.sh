#!/usr/bin/env bash
set -euo pipefail

platform=${1:-}
build_id=${2:-}
if [[ ! "$platform" =~ ^(android|ios)$ || -z "$build_id" ]]; then
  echo "Usage: bash ../../scripts/expo-simulator-smoke.sh <android|ios> <EAS build ID> [screenshot path]" >&2
  exit 2
fi
if [[ -s .env.eas-simulator ]]; then
  echo "A local simulator session configuration already exists. Use a clean checkout for this smoke test." >&2
  exit 2
fi

screenshot=${3:-.expo/simulator-smoke-$platform.png}
mkdir -p "$(dirname "$screenshot")"

eas() {
  npm exec --yes --package=eas-cli@27.11.0 -- eas "$@"
}

eas simulator:availability --json | jq -e '.available == true' >/dev/null

cleanup() {
  if [[ -s .env.eas-simulator ]]; then
    eas simulator:stop --non-interactive >/dev/null && rm .env.eas-simulator
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

eas simulator:start --platform "$platform" --type agent-device --build-id "$build_id" \
  --name "Socium $platform smoke" --max-duration-minutes 12 --max-idle-time-minutes 5 \
  --json >/dev/null

ready=false
for attempt in {1..12}; do
  if eas simulator:get --json | jq -e '.status == "IN_PROGRESS" and .remoteConfig != null' >/dev/null; then
    ready=true
    break
  fi
  sleep 5
done
if [[ "$ready" != true ]]; then
  echo "Expo simulator did not become ready." >&2
  exit 1
fi

eas simulator:exec env -u EXPO_TOKEN npx --yes agent-device@0.21.41 test --maestro .maestro/connect.yaml --platform "$platform"
eas simulator:exec env -u EXPO_TOKEN npx --yes agent-device@0.21.41 screenshot "$screenshot" --platform "$platform"
