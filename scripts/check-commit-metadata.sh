#!/usr/bin/env bash
# Rejects commit metadata that must not be published: links to Claude Code
# sessions, and the maintainer's commits under an address other than the
# GitHub noreply one.
#
# usage: check-commit-metadata.sh --message FILE   (commit-msg hook)
#        check-commit-metadata.sh RANGE            (CI, e.g. origin/main..HEAD)
set -euo pipefail

maintainer=chansuke
noreply=501052+chansuke@users.noreply.github.com
session='claude\.ai/code/session|^Claude-Session:'
fail=0

check_ident() { # NAME EMAIL WHAT
  if [ "$1" = "$maintainer" ] && [ "$2" != "$noreply" ]; then
    echo "$3: $maintainer must commit as $noreply" >&2
    fail=1
  fi
}

# Whether TEXT links a session. grep exits 1 for no match; any other
# failure stops the check rather than passing it.
links_session() { # TEXT
  local status=0
  grep -Eiq "$session" <<<"$1" || status=$?
  [ "$status" -le 1 ] || exit 2
  [ "$status" -eq 0 ]
}

case "${1:-}" in
--message)
  message=$(cat -- "${2:?usage: $0 --message FILE}")
  if links_session "$message"; then
    echo "The commit message links a Claude Code session; remove that line." >&2
    fail=1
  fi
  for var in GIT_AUTHOR_IDENT GIT_COMMITTER_IDENT; do
    ident=$(git var "$var")
    email=${ident#*<}
    check_ident "${ident% <*}" "${email%%>*}" "$var"
  done
  ;;
"")
  echo "usage: $0 --message FILE | RANGE" >&2
  exit 2
  ;;
*)
  commits=$(git log --format=$'%H\t%an\t%ae\t%cn\t%ce' "$1")
  while IFS=$'\t' read -r sha author_name author_email committer_name committer_email; do
    [ -n "$sha" ] || continue
    check_ident "$author_name" "$author_email" "$sha author"
    check_ident "$committer_name" "$committer_email" "$sha committer"
    if links_session "$(git log -1 --format=%B "$sha")"; then
      echo "$sha: the message links a Claude Code session" >&2
      fail=1
    fi
  done <<<"$commits"
  ;;
esac
exit "$fail"
