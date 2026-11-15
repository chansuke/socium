#!/usr/bin/env bash
# Runs the 2026-11-14 review observations against a temporary export of one
# commit. Nothing is written to the repository: sources are exported with
# `git archive`, builds go to a temporary target directory, and logs go to
# OUT (a new temporary directory unless given).
#
#   docs/review-evidence/2026-11-14/run_repros.sh [COMMIT] [OUT]
#
# Needs Nix (for the pinned toolchain and bubblewrap) and, on Ubuntu, the
# AppArmor userns profile for that bubblewrap. No API key is read and no
# request leaves the machine. The whole run takes about five minutes; the
# timeout observation alone takes 95 seconds.
#
# Keep TMPDIR short (the default is fine). With a long TMPDIR the existing
# test execution::tests::network_and_kernel_escape_interfaces_are_denied
# fails while binding a Unix socket whose path exceeds SUN_LEN; see
# baseline-tests-long-tmpdir.log.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
commit=${1:-2d4ac914051e974efa24b926d96e9b42e97904d5}
out=${2:-$(mktemp -d -t socium-review-logs.XXXXXX)}
work=$(mktemp -d -t socium-review-src.XXXXXX)
mkdir -p "$out"

export_tree() {
  mkdir -p "$1"
  git -C "$repo" archive "$commit" | tar -x -C "$1"
}

add_repros() {
  cp "$here/review_repros.rs" "$1/crates/core/src/review_repros_2026_11_14.rs"
  printf '\n#[cfg(test)]\nmod review_repros_2026_11_14;\n' >> "$1/crates/core/src/lib.rs"
}

run() {
  # run <tree> <target dir> <log> <cargo test arguments...>
  local tree=$1 target=$2 log=$3
  shift 3
  (cd "$tree" && CARGO_TARGET_DIR="$target" nix develop "$repo" -c cargo test --locked "$@") \
    > "$out/$log" 2>&1 || echo "cargo test exited with $?" >> "$out/$log"
}

# 1. The commit as it is.
export_tree "$work/base"
run "$work/base" "$work/target" baseline-tests.log --workspace --all-targets --all-features

# 2. The same tree with the observation module added.
export_tree "$work/repro"
add_repros "$work/repro"
run "$work/repro" "$work/target" repros.log -p socium-core review_ -- \
  --nocapture --test-threads=1 --skip review_r12
run "$work/repro" "$work/target" repro-r1-http-timeout.log -p socium-core review_r1_ -- \
  --nocapture --ignored
run "$work/repro" "$work/target" timings-release.log --release -p socium-core -- \
  --nocapture --test-threads=1 review_r12 review_r2

# 3. Control for R6: the same tree with socketpair removed from the seccomp
#    deny list and nothing else changed.
cp -r "$work/repro" "$work/control"
sed -i '/^            libc::SYS_socketpair,$/d' "$work/control/crates/core/src/execution/linux.rs"
diff "$work/repro/crates/core/src/execution/linux.rs" \
  "$work/control/crates/core/src/execution/linux.rs" > "$out/control-socketpair.diff" || true
run "$work/control" "$work/target-control" repro-r6-socketpair-allowed.log -p socium-core review_r6 -- \
  --nocapture --test-threads=1

echo "commit: $commit" > "$out/run-info.txt"
echo "logs: $out"
echo "sources and build output (safe to delete): $work"
