#!/usr/bin/env bash
# Fails when docs/feature-map.md names a path or a test that does not exist,
# or when a TUI scenario is missing from it.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
map="$root/docs/feature-map.md"
fail=0
# shellcheck disable=SC2016 # the backticks are literal
tokens=$(grep -o '`[^`]*`' "$map" | tr -d '`' | sort -u)

while IFS= read -r token; do
  case $token in
  crates/* | scripts/* | docs/* | evals/* | bench/*)
    # A command such as `scripts/verify tui` names the script.
    path=${token%% *}
    if [ ! -e "$root/$path" ]; then
      echo "docs/feature-map.md names $path, which does not exist. Point the row at where the code is now." >&2
      fail=1
    fi
    ;;
  *)
    # A test: snake_case with at least three words.
    if [[ $token =~ ^[a-z0-9]+(_[a-z0-9]+){2,}$ ]] &&
      ! grep -rqE --include='*.rs' "fn $token\(" "$root/crates"; then
      echo "docs/feature-map.md names the test $token, which does not exist. Name the test that checks the feature now." >&2
      fail=1
    fi
    ;;
  esac
done <<<"$tokens"

for scenario in "$root"/crates/tui/scenarios/*.toml; do
  relative=${scenario#"$root"/}
  if ! grep -qF "\`$relative\`" "$map"; then
    echo "$relative is not in docs/feature-map.md. Add it to the row of the feature it shows." >&2
    fail=1
  fi
done

exit "$fail"
