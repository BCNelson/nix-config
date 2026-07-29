#!/bin/bash
set -euo pipefail

# Build the system closure for every thin client, sign it, publish it to the
# local binary cache directory, and write a manifest the client polls.
#
# Runs on the builder immediately after its own auto-update lands, so the
# closures a thin client can see always correspond to a commit this machine has
# already proven it can build.

for var in CONFIG_PATH CACHE_DIR MANIFEST_DIR SIGNING_KEY_FILE; do
    if [ -z "${!var-}" ]; then
        echo "$var not set" >&2
        exit 1
    fi
done

if [ -z "${THIN_HOSTS-}" ]; then
    echo "No thin clients configured; nothing to build"
    exit 0
fi

if [ ! -r "$SIGNING_KEY_FILE" ]; then
    echo "Signing key $SIGNING_KEY_FILE is missing or unreadable" >&2
    exit 1
fi

GCROOT_DIR=/nix/var/nix/gcroots/thin-clients

cd "$CONFIG_PATH"

git config --global --add safe.directory "$CONFIG_PATH"

if ! git config --local --get filter.git-crypt.smudge > /dev/null; then
    echo "$CONFIG_PATH is locked and must be git-crypt unlocked before building" >&2
    exit 1
fi

commit=$(git rev-parse HEAD)
echo "Building thin client closures at $commit"

mkdir -p "$CACHE_DIR" "$MANIFEST_DIR" "$GCROOT_DIR"

# Write a manifest atomically: the client may read it at any moment and a
# half-written file would parse as a failure.
write_manifest() {
    local host="$1" status="$2" storePath="$3" error="$4"
    local tmp
    tmp=$(mktemp "$MANIFEST_DIR/.$host.XXXXXX")
    jq -n \
        --arg hostname "$host" \
        --arg commit "$commit" \
        --arg storePath "$storePath" \
        --arg status "$status" \
        --arg error "$error" \
        --arg builtAt "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        '{hostname: $hostname, commit: $commit, storePath: $storePath, status: $status, builtAt: $builtAt}
         + (if $error == "" then {} else {error: $error} end)' > "$tmp"
    chmod 0644 "$tmp"
    mv -f "$tmp" "$MANIFEST_DIR/$host.json"
}

# The storePath a previous run already published for this host, so an unchanged
# host is skipped without re-copying its whole closure.
published_path() {
    local host="$1"
    [ -f "$MANIFEST_DIR/$host.json" ] || return 0
    jq -r 'select(.status == "ready") | .storePath // empty' "$MANIFEST_DIR/$host.json" 2>/dev/null || true
}

failed=0

for host in $THIN_HOSTS; do
    echo "=== $host ==="

    # --out-link into the gcroots directory registers a permanent root, so this
    # machine's own garbage collection cannot delete a closure that a manifest
    # still advertises before the client has had a chance to fetch it.
    link="$GCROOT_DIR/$host"
    buildLog=$(mktemp)
    if ! nix build --out-link "$link" \
            ".#nixosConfigurations.$host.config.system.build.toplevel" 2>&1 | tee "$buildLog"; then
        echo "Failed to build $host"
        write_manifest "$host" "failed" "" "$(tail -c 2000 "$buildLog")"
        rm -f "$buildLog"
        failed=1
        continue
    fi
    rm -f "$buildLog"

    out=$(readlink -f "$link")

    if [ "$out" == "$(published_path "$host")" ]; then
        echo "$host unchanged ($out); refreshing manifest only"
        write_manifest "$host" "ready" "$out" ""
        continue
    fi

    echo "Publishing $out"
    # zstd rather than the default xz: these are large closures republished on
    # every config change to clients on the LAN, so compression time costs more
    # than the saved bytes are worth.
    if ! nix copy --to "file://$CACHE_DIR?secret-key=$SIGNING_KEY_FILE&compression=zstd" "$out"; then
        echo "Failed to publish $host to $CACHE_DIR"
        write_manifest "$host" "failed" "" "nix copy to the binary cache failed"
        failed=1
        continue
    fi

    write_manifest "$host" "ready" "$out" ""
    echo "$host ready: $out"
done

# Drop roots for hosts that are no longer thin clients, so their closures can
# eventually be collected.
for link in "$GCROOT_DIR"/*; do
    [ -e "$link" ] || continue
    host=$(basename "$link")
    if ! grep -qw "$host" <<<"$THIN_HOSTS"; then
        echo "Releasing stale gcroot for $host"
        rm -f "$link" "$MANIFEST_DIR/$host.json"
    fi
done

exit "$failed"
