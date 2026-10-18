#!/usr/bin/env bash
# Tests for advance-auto-update.sh against a throwaway bare repository.
# Run from anywhere: .forgejo/scripts/test-advance-auto-update.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
advance="$here/advance-auto-update.sh"
real_git=$(command -v git)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

srv="$tmp/srv.git"
git init -q --bare "$srv"
git -C "$srv" config uploadpack.allowFilter true
git -C "$srv" config uploadpack.allowAnySHA1InWant true
export REMOTE="file://$srv"

# History: A - B - C - D on main, plus X branching off A.
git init -q "$tmp/w"
# Real trees and blobs, so the push from the treeless scratch repo is exercised.
commit() {
  echo "$1" >"$tmp/w/$1.txt"
  git -C "$tmp/w" add "$1.txt"
  git -C "$tmp/w" commit -q -m "$1"
  git -C "$tmp/w" rev-parse HEAD
}
A=$(commit A)
B=$(commit B)
C=$(commit C)
D=$(commit D)
git -C "$tmp/w" checkout -q -b side "$A"
X=$(commit X)
git -C "$tmp/w" push -q "$srv" "$D:refs/heads/main" "$X:refs/heads/side"

set_branch() { git -C "$srv" update-ref refs/heads/auto-update "$1"; }
branch_at() { git -C "$srv" rev-parse refs/heads/auto-update 2>/dev/null || echo missing; }

failures=0
check() { # name expected-exit expected-ref expected-advanced sha
  local name=$1 want_rc=$2 want_ref=$3 want_adv=$4 sha=$5 rc=0 out="$tmp/out"
  : >"$out"
  GITHUB_OUTPUT="$out" "$advance" "$sha" >"$tmp/log" 2>&1 || rc=$?
  local got_ref got_adv
  got_ref=$(branch_at)
  got_adv=$(sed -n 's/^advanced=//p' "$out")
  if [ "$rc" = "$want_rc" ] && [ "$got_ref" = "$want_ref" ] && [ "$got_adv" = "$want_adv" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: rc=$rc (want $want_rc) ref=$got_ref (want $want_ref) advanced=$got_adv (want $want_adv)"
    sed 's/^/     /' "$tmp/log"
    failures=$((failures + 1))
  fi
}

git -C "$srv" update-ref -d refs/heads/auto-update 2>/dev/null || true
check "creates a missing branch" 0 "$A" true "$A"

set_branch "$A"
check "fast-forwards A -> C" 0 "$C" true "$C"

set_branch "$C"
check "older B after C is a no-op" 0 "$C" false "$B"

set_branch "$C"
check "same commit is a no-op" 0 "$C" false "$C"

set_branch "$C"
check "diverged X is refused" 1 "$C" "" "$X"

# Race: something advances auto-update between our read and our push. A git
# shim moves the server's ref just before the first push, so the lease must
# reject it and the retry must re-evaluate against the new value.
race() { # name moved-to sha expected-ref expected-advanced
  local shim="$tmp/shim"
  rm -rf "$shim"
  mkdir -p "$shim"
  cat >"$shim/git" <<EOF
#!/usr/bin/env bash
if [ "\$1" = push ] && [ ! -e "$shim/moved" ]; then
  touch "$shim/moved"
  "$real_git" -C "$srv" update-ref refs/heads/auto-update "$2"
fi
exec "$real_git" "\$@"
EOF
  chmod +x "$shim/git"
  set_branch "$A"
  PATH="$shim:$PATH" check "$1" 0 "$4" "$5" "$3"
}
race "race: moved past us to D, advancing to C is a no-op" "$D" "$C" "$D" false
race "race: moved to B, still fast-forwards to C" "$B" "$C" "$C" true

if [ "$failures" -gt 0 ]; then
  echo "$failures test(s) failed"
  exit 1
fi
echo "all tests passed"
