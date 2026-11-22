#!/usr/bin/env bash
# Release notes for a tag, written from the conventional-commit prefixes of the commits made since the
# previous tag. Usage: scripts/release-notes.sh vX.Y.Z   (prints Markdown on stdout)
#
# feat -> Added, fix -> Fixed, security -> Security, perf -> Performance, a "!" after the type or a
# "BREAKING CHANGE" footer -> Breaking changes. docs, test, chore, ci and refactor are left out, as in
# .releaserc.json; a commit without a known prefix goes under Other. The first tag lists everything.
set -euo pipefail

tag=${1:?usage: release-notes.sh <tag>}
prev=$(git describe --tags --abbrev=0 --match 'v*' "${tag}^" 2>/dev/null || true)
range=${prev:+$prev..}$tag

declare -A section=(
  [breaking]="" [feat]="" [fix]="" [security]="" [perf]="" [other]=""
)
repo=${GITHUB_REPOSITORY:-}

while IFS=$'\x1f' read -r hash subject body; do
  [ -n "$hash" ] || continue
  re='^([a-z]+)(\(([^)]*)\))?(!)?: (.+)$'
  type=other scope="" text=$subject bang=""
  if [[ $subject =~ $re ]]; then
    type=${BASH_REMATCH[1]} scope=${BASH_REMATCH[3]} bang=${BASH_REMATCH[4]} text=${BASH_REMATCH[5]}
  fi
  case $type in
    docs | test | chore | ci | refactor | style | build | revert) [ -n "$bang" ] || continue ;;
    feat | fix | security | perf) ;;
    *) type=other ;;
  esac
  line="- "
  [ -n "$scope" ] && line+="**$scope:** "
  line+="$text (\`${hash:0:7}\`)"$'\n'
  if [ -n "$bang" ] || [[ $body == *"BREAKING CHANGE"* ]]; then
    section[breaking]+=$line
  fi
  case $type in feat | fix | security | perf | other) section[$type]+=$line ;; esac
done < <(git log --no-merges --format='%h%x1f%s%x1f%b%x1e' "$range" | tr -d '\n' | tr '\036' '\n' | sed 's/\x1f/\x1f/g')

print() { [ -n "${section[$1]}" ] && printf '## %s\n\n%s\n' "$2" "${section[$1]}"; return 0; }
print breaking "Breaking changes"
print feat "Added"
print fix "Fixed"
print security "Security"
print perf "Performance"
print other "Other"
if [ -z "${section[breaking]}${section[feat]}${section[fix]}${section[security]}${section[perf]}${section[other]}" ]; then
  echo "No user-facing changes."
  echo
fi
if [ -n "$repo" ] && [ -n "$prev" ]; then
  echo "**Full changelog:** https://github.com/$repo/compare/$prev...$tag"
fi
