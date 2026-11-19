#!/usr/bin/env bash
# Tests check-commit-metadata.sh against a throwaway repository.
set -euo pipefail

check=$(cd "$(dirname "$0")" && pwd)/check-commit-metadata.sh
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT
cd "$repo"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -q -b main
noreply=501052+chansuke@users.noreply.github.com
commit() { # NAME EMAIL MESSAGE
  GIT_AUTHOR_NAME=$1 GIT_AUTHOR_EMAIL=$2 GIT_COMMITTER_NAME=$1 GIT_COMMITTER_EMAIL=$2 \
    git commit -q --allow-empty -m "$3"
}
failures=0
expect() { # WANT(pass|fail) DESCRIPTION COMMAND...
  local want=$1 what=$2 status=0
  shift 2
  "$@" 2>/dev/null || status=$?
  if { [ "$want" = pass ] && [ "$status" -ne 0 ]; } || { [ "$want" = fail ] && [ "$status" -ne 1 ]; }; then
    echo "FAIL: $what (exit $status)"
    failures=1
  fi
}

commit chansuke "$noreply" "feat: base"
commit someone someone@example.com "fix: by another contributor"
expect pass "clean history" "$check" main

commit chansuke "$noreply" $'fix: x\n\nClaude-Session: https://claude.ai/code/session_x'
expect fail "a session line" "$check" main
expect pass "a range before it" "$check" main~1
git reset -q --hard HEAD~1

GIT_AUTHOR_NAME=chansuke GIT_AUTHOR_EMAIL=personal@example.com \
  GIT_COMMITTER_NAME=chansuke GIT_COMMITTER_EMAIL=$noreply git commit -q --allow-empty -m "fix: authored elsewhere"
expect fail "the maintainer's authorship under another address" "$check" main
git reset -q --hard HEAD~1

GIT_AUTHOR_NAME=chansuke GIT_AUTHOR_EMAIL=$noreply \
  GIT_COMMITTER_NAME=chansuke GIT_COMMITTER_EMAIL=personal@example.com git commit -q --allow-empty -m "fix: committed elsewhere"
expect fail "the maintainer's commit under another address" "$check" main
git reset -q --hard HEAD~1

expect fail "an unknown range is an error, not a pass" test "$("$check" no-such-ref >/dev/null 2>&1; echo $?)" = 0
expect fail "no arguments is an error, not a pass" test "$("$check" >/dev/null 2>&1; echo $?)" = 0

printf 'fix: x\n\nsee https://claude.ai/code/session_y\n' >bad
printf 'fix: x\n' >good
printf 'fix: x\n\nsee https://CLAUDE.AI/code/session_z\n' >shouting
export GIT_AUTHOR_NAME=chansuke GIT_AUTHOR_EMAIL=$noreply GIT_COMMITTER_NAME=chansuke GIT_COMMITTER_EMAIL=$noreply
expect fail "a message with a session link" "$check" --message bad
expect pass "a plain message" "$check" --message good
expect fail "a session link in capitals" "$check" --message shouting
expect fail "an unreadable message is an error, not a pass" test "$("$check" --message missing >/dev/null 2>&1; echo $?)" = 0
GIT_AUTHOR_EMAIL=personal@example.com expect fail "the author under another address" "$check" --message good
GIT_COMMITTER_EMAIL=personal@example.com expect fail "the committer under another address" "$check" --message good

[ "$failures" -eq 0 ] && echo "check-commit-metadata: all cases pass"
exit "$failures"
