#!/usr/bin/env bash
# Dry-run every nixosConfiguration. Used by check.yml and update.yml. Run
# inside nixos/nix: one process store, so flake inputs are fetched once.
#
# Evaluation is single-threaded and ~2.5G per host, so hosts run CHECK_JOBS at
# a time (default 6) -- one after another took ~7.5 minutes for 17 hosts while
# the rest of romeo's 32 cores sat idle. Measured on romeo: 4 jobs 169s, 6 jobs
# 147s, 8 jobs 137s. Past 6 the wall time is iso_console (~130s, it builds the
# patched nixpkgs) plus the first wave fetching inputs, so more jobs mostly
# just cost memory. Each host's output is buffered and
# kept and printed in host order once all are done (a one-line status as each
# finishes), so parallel output never interleaves.
#
# path: rather than the git checkout, since the clone is shallow. Locally
# (`nixcfg check-host --all`) CHECK_FLAKE=. evaluates the git checkout instead,
# including uncommitted changes to tracked files, without copying untracked
# clutter (VM images, build results) into the store.
set -uo pipefail

flake="${CHECK_FLAKE:-path:$PWD}"
parallel="${CHECK_JOBS:-6}"
logs=$(mktemp -d)

# Inputs are fetched lazily by whichever evaluation needs them first; the
# others wait on the store path lock rather than downloading again. Don't
# `nix flake archive` up front: it fetches all ~86 transitive inputs, most of
# which no host evaluates, and took 147s on its own.
hosts=$(nix eval --raw "$flake#nixosConfigurations" --apply 'c: builtins.concatStringsSep " " (builtins.attrNames c)')
# Slowest first, so they don't end up alone at the tail: iso_console builds a
# patched nixpkgs, and the desktops evaluate far more than the servers.
order=$(printf '%s\n' $hosts | grep -x iso_console; printf '%s\n' $hosts | grep -vx iso_console)

check() {
  local name=$1 attr=$2 start=$SECONDS result=FAILED
  # --no-eval-cache: it is keyed on the source tree, so it never hits for a new
  # commit, and parallel evaluations just contend on its SQLite lock.
  if nix build "$flake#$attr" --dry-run --show-trace --no-eval-cache >"$logs/$name" 2>&1; then
    touch "$logs/$name.ok"
    result=ok
  fi
  echo "$name: $result ($((SECONDS - start))s)"
}

for host in $order devShell; do
  if [ "$host" = devShell ]; then
    attr="devShells.x86_64-linux.default"
  else
    attr="nixosConfigurations.$host.config.system.build.toplevel"
  fi
  while [ "$(jobs -rp | wc -l)" -ge "$parallel" ]; do
    wait -n
  done
  check "$host" "$attr" &
done
wait

failed=""
for host in $order devShell; do
  echo "::group::$host"
  cat "$logs/$host"
  echo "::endgroup::"
  [ -e "$logs/$host.ok" ] || failed="$failed $host"
done

if [ -n "$failed" ]; then
  echo "Failed:$failed"
  exit 1
fi
echo "All hosts evaluated: $hosts"
