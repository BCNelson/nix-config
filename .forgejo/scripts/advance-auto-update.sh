#!/usr/bin/env bash
# Advance the auto-update branch to a checked commit -- forward only.
#
# Every auto-updating host builds auto-update, so it must never move
# backwards. It used to be force-pushed by each check.yml run, and runs are not
# serialised: a slow run for an older commit could finish after a newer one and
# roll every host back to it. Instead, compare against the branch as it is now:
#
#   auto-update is an ancestor of SHA  -> fast-forward it (lease-guarded)
#   SHA is already in auto-update      -> nothing to do; a newer commit shipped
#   neither (history was rewritten)    -> fail and leave it to a human
#
# The push carries a lease on the value we compared against, so a concurrent
# advance between our read and our push is rejected and re-evaluated rather
# than overwritten.
#
# Usage: REMOTE=<url with credentials> advance-auto-update.sh <sha>
# Writes advanced=true|false to $GITHUB_OUTPUT so the ntfy ping only fires when
# hosts actually have something new to build.
set -euo pipefail

sha=${1:?usage: advance-auto-update.sh <sha>}
remote=${REMOTE:?REMOTE must be set to the repository URL}
branch=${BRANCH:-auto-update}
ref="refs/heads/$branch"
out=${GITHUB_OUTPUT:-/dev/null}
attempts=5

# Work in a scratch repository so this behaves the same whatever the caller
# has checked out (a shallow clone cannot answer ancestry questions).
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git init -q "$work"
cd "$work"
git remote add origin "$remote"

# Commits only, no trees or blobs: ancestry is all we need, and it keeps the
# fetch cheap. promisor lets git fetch anything else lazily if push wants it.
git config remote.origin.promisor true
git config remote.origin.partialclonefilter tree:0

fetch() {
  if ! git fetch -q --filter=tree:0 origin "$@" 2>/dev/null; then
    echo "Filtered fetch refused; falling back to a full fetch"
    git fetch -q origin "$@"
  fi
}

for attempt in $(seq 1 "$attempts"); do
  old=$(git ls-remote origin "$ref" | cut -f1)
  if [ -n "$old" ]; then
    fetch "$sha" "$old"
  else
    fetch "$sha"
  fi
  sha=$(git rev-parse --verify "$sha^{commit}")

  if [ -n "$old" ]; then
    if [ "$old" = "$sha" ] || git merge-base --is-ancestor "$sha" "$old"; then
      echo "$branch is already at $old, which contains $sha; nothing to do"
      echo "advanced=false" >>"$out"
      exit 0
    fi
    if ! git merge-base --is-ancestor "$old" "$sha"; then
      base=$(git merge-base "$old" "$sha" || echo none)
      echo "Refusing to move $branch: $old and $sha have diverged (merge-base $base)."
      echo "main's history was probably rewritten; move $branch by hand once you have checked which commit hosts should run."
      exit 1
    fi
    echo "Fast-forwarding $branch $old -> $sha"
  else
    echo "Creating $branch at $sha"
  fi

  # An empty lease value means "the ref must not exist yet".
  if git push -q --force-with-lease="$ref:$old" origin "$sha:$ref"; then
    echo "advanced=true" >>"$out"
    exit 0
  fi
  echo "Push rejected (attempt $attempt/$attempts); $branch moved underneath us, re-checking"
done

echo "Gave up advancing $branch after $attempts attempts"
exit 1
