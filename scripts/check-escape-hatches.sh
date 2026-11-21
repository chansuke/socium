#!/usr/bin/env bash
# Rejects added lines that put off a fix or switch a check off without a
# reason: TODO-style markers and "workaround" comments, `#[ignore]` without
# a reason, and linter or type-checker suppressions without one. Only the
# lines a change adds are read, so existing code never fails it.
#
# usage: check-escape-hatches.sh --staged     (pre-commit: the index against HEAD)
#        check-escape-hatches.sh BASE [HEAD]  (CI: BASE to HEAD, or to the working tree)
set -euo pipefail

case "${1:-}" in
--staged) diff=(git diff --cached --no-color --no-ext-diff -U0 --) ;;
"")
  echo "usage: $0 --staged | BASE [HEAD]" >&2
  exit 2
  ;;
*) diff=(git diff --no-color --no-ext-diff -U0 "$@" --) ;;
esac

# Each rule: a file pattern (a shell glob in which `*` also matches `/`), an
# extended regular expression for the added text, and what to do instead.
# Comment rules read only the comment part of a line; see `comment_of`.
rules=(
  $'comment\t*\t(^|[^A-Za-z])(TODO|FIXME|HACK|XXX)([^A-Za-z]|$)\tFix it in this change. If it is out of scope, open an issue and describe what the code does now as a fact, without a promise; better, add a test or a lint that fails until it is fixed.'
  $'comment\t*\t[Ww]ork-?around|[Ff]or now|[Qq]uick fix|[Bb]and-?aid|[Tt]emporary (fix|hack|patch)|[Uu]ntil .* (is|gets|are) fixed\tDo not explain away a workaround. Find the cause and fix it; if the cause is outside this repository, say which upstream behavior the code handles and link the upstream issue.'
  $'code\t*.rs\t#\\[ignore\\]\tGive the reason: #[ignore = "why it does not run by default"]. A test that fails is fixed or deleted, not ignored.'
  $'code\t*.py\t#[[:space:]]*noqa([^:]|$)|#[[:space:]]*type:[[:space:]]*ignore([^[]|$)\tName the rule, as `# noqa: E402` or `# type: ignore[attr-defined]`, or change the code so the checker passes.'
  $'code\t*.sh\t#[[:space:]]*shellcheck[[:space:]]+disable=[A-Z0-9,]+[[:space:]]*$\tSay why after the directive, as this repository does: `# shellcheck disable=SC2016 # the backticks are literal`.'
)

# The comment part of LINE in FILE, empty when it has none. Rust and Nix
# use `//` and `#`; a `#` that starts an attribute such as `#[cfg]` is code.
comment_of() { # FILE LINE
  case $1 in
  *.rs) [[ $2 == *//* ]] && printf '%s' "//${2#*//}" ;;
  *.py | *.sh | *.toml | *.yaml | *.yml | *.nix | Makefile | *.mk)
    [[ $2 =~ (^|[[:space:]])#([^[!]|$) ]] && printf '%s' "#${2#*#}"
    ;;
  esac
  return 0
}

# Text files only: prose in docs may say "for now", and fixtures may hold
# anything.
checked() { # FILE
  case $1 in
  docs/* | *.md | *.lock | *.example | evals/tasks/*) return 1 ;;
  scripts/check-escape-hatches.sh | scripts/test-check-escape-hatches.sh) return 1 ;;
  esac
  return 0
}

found=0
file=""
line=0
while IFS= read -r row; do
  case $row in
  '+++ b/'*)
    file=${row#+++ b/}
    continue
    ;;
  '+++ '* | '--- '*) continue ;;
  '@@ '*)
    # @@ -a,b +c,d @@: the next added line is line c of the new file.
    line=${row#*+}
    line=${line%%[, ]*}
    continue
    ;;
  '+'*) ;;
  *) continue ;;
  esac
  text=${row#+}
  if checked "$file"; then
    comment=$(comment_of "$file" "$text")
    for rule in "${rules[@]}"; do
      IFS=$'\t' read -r kind glob pattern advice <<<"$rule"
      # shellcheck disable=SC2053 # the pattern is meant to glob
      [[ $file == $glob ]] || continue
      subject=$text
      [ "$kind" = comment ] && subject=$comment
      if [ -n "$subject" ] && grep -Eq -- "$pattern" <<<"$subject"; then
        printf '%s:%s: %s\n    %s\n' "$file" "$line" "${text#"${text%%[![:space:]]*}"}" "$advice" >&2
        found=$((found + 1))
      fi
    done
  fi
  line=$((line + 1))
done < <("${diff[@]}")

if ((found > 0)); then
  echo "$found added line(s) put off a fix or switch off a check without a reason." >&2
  exit 1
fi
