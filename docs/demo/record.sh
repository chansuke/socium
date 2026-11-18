#!/usr/bin/env bash
# Records docs/assets/demo.gif and docs/assets/demo.png from demo.tape.
# The model is provider.py on loopback, so nothing is sent to a real provider.
#
# usage: nix develop -c docs/demo/record.sh
# Needs target/release/socium (cargo build --release -p socium-tui), or the
# binary named by SOCIUM_BIN.
set -euo pipefail

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
demo=$repo/docs/demo
work=$(mktemp -d /tmp/demo.XXXX)
trap 'kill "${provider:-}" 2>/dev/null; rm -rf "$work"' EXIT

mkdir -p "$work/agents" "$work/bin"
cp -r "$demo/workspace" "$work/slug"
git -C "$work/slug" init -q
git -C "$work/slug" -c user.name=demo -c user.email=demo@example.com add .
git -C "$work/slug" -c user.name=demo -c user.email=demo@example.com commit -qm "Add slugify"
ln -s "${SOCIUM_BIN:-$repo/target/release/socium}" "$work/bin/socium"

python3 -I "$demo/provider.py" --port-file "$work/port" &
provider=$!
for _ in $(seq 50); do [ -s "$work/port" ] && break; sleep 0.1; done

cat > "$work/models.toml" <<TOML
default = "demo"

[providers.demo]
wire = "openai-chat"
base_url = "http://127.0.0.1:$(cat "$work/port")/v1"
api_key_env = "DEMO_API_KEY"

[[models]]
alias = "demo"
provider = "demo"
id = "demo-model"
TOML
printf '[approval]\ncommands = "ask"\n' > "$work/config.toml"

cd "$repo"
env -i \
  HOME="$work" \
  PATH="$work/bin:$(dirname "$(command -v bwrap)"):/usr/bin:/bin" \
  TERM=xterm-256color LANG=C.UTF-8 \
  DEMO_WS="$work/slug" DEMO_API_KEY=unused MODEL=demo \
  SOCIUM_MODELS_FILE="$work/models.toml" \
  SOCIUM_CONFIG_FILE="$work/config.toml" \
  SOCIUM_AGENTS_DIR="$work/agents" \
  "${VHS:-$(nix build --no-link --print-out-paths nixpkgs#vhs)/bin/vhs}" "$demo/demo.tape"
