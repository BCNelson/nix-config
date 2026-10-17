#!/usr/bin/env bash
# Dry-run every nixosConfiguration, one after another. Used by check.yml and
# update.yml. Run inside nixos/nix: one process store, so flake inputs are
# fetched once, and romeo isn't asked to evaluate eight hosts at a time.
#
# path: rather than the git checkout, since the clone is shallow.
set -uo pipefail

flake="path:$PWD"
hosts=$(nix eval --raw "$flake#nixosConfigurations" --apply 'c: builtins.concatStringsSep " " (builtins.attrNames c)')
failed=""
for host in $hosts; do
  echo "::group::$host"
  if ! nix build "$flake#nixosConfigurations.$host.config.system.build.toplevel" --dry-run --show-trace; then
    failed="$failed $host"
  fi
  echo "::endgroup::"
done

echo "::group::devShell"
if ! nix build "$flake#devShells.x86_64-linux.default" --dry-run --show-trace; then
  failed="$failed devShell"
fi
echo "::endgroup::"

if [ -n "$failed" ]; then
  echo "Failed:$failed"
  exit 1
fi
echo "All hosts evaluated: $hosts"
