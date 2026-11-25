#!/usr/bin/env bash
# Release notes for a tag: the commits since the previous tag as a bulleted list, grouped into
# "Fixes" and "What's new", ending with the GitHub compare link. Shown in the in-app update window.
#
# Usage: tools/release/notes.sh <tag> [<repo as owner/name>]   (needs full git history and tags)
# Hidden from the list: version bumps, merges, and commits whose subject starts with "chore",
# "ci", "docs", "test" or "build" (or mentions the release plumbing).
set -euo pipefail

TAG="${1:?usage: notes.sh <tag> [owner/name]}"
REPO="${2:-}"

PREV=$(git describe --tags --abbrev=0 --match 'v*' "$TAG^" 2>/dev/null || true)
RANGE="${PREV:+$PREV..}$TAG"

fixes=""
news=""
while IFS= read -r subject; do
  [ -z "$subject" ] && continue
  case "$subject" in
    Bump\ version*|Merge\ *|Revert\ *) continue ;;
  esac
  lower=$(printf '%s' "$subject" | tr '[:upper:]' '[:lower:]')
  case "$lower" in
    chore*|ci:*|ci\ *|docs*|test*|build*|*macos-latest*|*runner*|*"build dependencies"*|*"release test"*) continue ;;
  esac
  # Trim a trailing PR/issue reference like " (#9)" and any trailing period.
  line=$(printf '%s' "$subject" | sed -E 's/ \(#[0-9]+\)$//; s/\.$//')
  # Capitalise the first letter.
  line="$(printf '%s' "${line:0:1}" | tr '[:lower:]' '[:upper:]')${line:1}"
  case "$lower" in
    fix*|*" fix "*|*" fixes "*|*" fixed "*|repair*|correct*|prevent*|stop\ *|no\ longer*|*crash*)
      fixes+="- ${line}"$'\n' ;;
    *)
      news+="- ${line}"$'\n' ;;
  esac
done < <(git log --no-merges --reverse --format='%s' "$RANGE")

out=""
[ -n "$news" ] && out+=$'## What\'s new\n'"$news"$'\n'
[ -n "$fixes" ] && out+=$'## Fixes\n'"$fixes"$'\n'
[ -z "$out" ] && out=$'Maintenance release.\n\n'
if [ -n "$REPO" ] && [ -n "$PREV" ]; then
  out+="**Full Changelog**: https://github.com/${REPO}/compare/${PREV}...${TAG}"$'\n'
fi
printf '%s' "$out"
