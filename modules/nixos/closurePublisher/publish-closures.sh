#!/bin/bash
set -euo pipefail

# Build the system closure for each configured host, sign it, copy it into the
# nginx-served binary cache directory, and publish a manifest naming the store
# path that is now current for that host.
#
# The point is that the *client* never evaluates or builds anything: it reads
# one line of text (the store path), substitutes the closure from this cache,
# and activates it. That is what lets a host like a Wyse 3040 thin client —
# 2 GiB of RAM, no room to evaluate this flake let alone build it — stay out
# of the build path entirely, both when updating (see
# services.bcnelson.remoteUpdate) and when first installed (see
# `install-system --limited`).

: "${FLAKE_PATH:?FLAKE_PATH not set}"
: "${CACHE_DIR:?CACHE_DIR not set}"
: "${MANIFEST_SUBDIR:?MANIFEST_SUBDIR not set}"
: "${TARGET_HOSTS:?TARGET_HOSTS not set}"
: "${SIGNING_KEY_FILE:?SIGNING_KEY_FILE not set}"
: "${SIGNING_KEY_NAME:?SIGNING_KEY_NAME not set}"
: "${RETENTION_DAYS:?RETENTION_DAYS not set}"

read -ra targetHosts <<< "$TARGET_HOSTS"

if [ ${#targetHosts[@]} -eq 0 ]; then
    echo "No hosts configured to publish; nothing to do."
    exit 0
fi

manifestDir="$CACHE_DIR/$MANIFEST_SUBDIR"
publicKeyFile="$SIGNING_KEY_FILE.pub"

install -d -m 0755 "$CACHE_DIR" "$manifestDir"

# Generate the cache signing key on first run. The private half never leaves
# this host; the public half is published next to the cache so clients can be
# configured with it (see `just cache-key`).
if [ ! -f "$SIGNING_KEY_FILE" ] || [ ! -f "$publicKeyFile" ]; then
    echo "Generating binary cache signing key $SIGNING_KEY_NAME"
    install -d -m 0700 "$(dirname "$SIGNING_KEY_FILE")"
    rm -f "$SIGNING_KEY_FILE" "$publicKeyFile"
    nix-store --generate-binary-cache-key "$SIGNING_KEY_NAME" "$SIGNING_KEY_FILE" "$publicKeyFile"
    chmod 0600 "$SIGNING_KEY_FILE"
fi
install -m 0444 "$publicKeyFile" "$CACHE_DIR/nix-cache-pubkey"

# The checkout is maintained by autoUpdate under a different user; without
# this, git refuses to read it for us and the flake fetch fails.
if ! git config --global --get-all safe.directory | grep -qxF "$FLAKE_PATH"; then
    git config --global --add safe.directory "$FLAKE_PATH"
fi

# Publish atomically: a client polling mid-copy must never see a path whose
# closure is only half in the cache.
publish_manifest() {
    local name="$1" path="$2"
    printf '%s\n' "$path" > "$manifestDir/.$name.tmp"
    chmod 0444 "$manifestDir/.$name.tmp"
    mv -f "$manifestDir/.$name.tmp" "$manifestDir/$name"
}

failed=0

for host in "${targetHosts[@]}"; do
    echo "=== $host: building system closure"
    if ! out=$(nix build --no-link --print-out-paths \
            "$FLAKE_PATH#nixosConfigurations.$host.config.system.build.toplevel" | tail -n1); then
        echo "$host: build failed" >&2
        failed=1
        continue
    fi

    if [ -z "$out" ]; then
        echo "$host: build produced no output path" >&2
        failed=1
        continue
    fi

    echo "=== $host: signing $out"
    nix store sign --key-file "$SIGNING_KEY_FILE" --recursive "$out"

    echo "=== $host: copying closure into $CACHE_DIR"
    nix copy --to "file://$CACHE_DIR?compression=zstd&parallel-compression=true" "$out"

    publish_manifest "$host" "$out"
    echo "=== $host: published $out"
done

# Drop cache entries that no published closure references any more. Without
# this the directory only ever grows, one full system closure per rebuild.
# Entries are only considered once they are older than RETENTION_DAYS so a
# client that started downloading the previous generation can finish.
prune_cache() {
    local live nar hash narinfo manifest
    live=$(mktemp)
    trap 'rm -f "$live" "$live.hashes"' RETURN

    for host in "${targetHosts[@]}"; do
        manifest="$manifestDir/$host"
        [ -f "$manifest" ] || continue
        # Bail out rather than prune on a partial view of what is live.
        if ! nix path-info --recursive "$(cat "$manifest")" >> "$live"; then
            echo "prune: could not resolve $manifest, skipping prune" >&2
            return 0
        fi
    done

    sed -nE 's|^/nix/store/([a-z0-9]{32})-.*$|\1|p' "$live" | sort -u > "$live.hashes"
    if [ ! -s "$live.hashes" ]; then
        echo "prune: no live paths resolved, skipping prune" >&2
        return 0
    fi

    while IFS= read -r -d '' narinfo; do
        hash=$(basename "$narinfo" .narinfo)
        if grep -qxF "$hash" "$live.hashes"; then
            continue
        fi
        nar=$(awk '/^URL: /{print $2; exit}' "$narinfo")
        rm -f "$narinfo"
        if [ -n "$nar" ]; then
            rm -f "$CACHE_DIR/$nar"
        fi
    done < <(find "$CACHE_DIR" -maxdepth 1 -type f -name '*.narinfo' -mtime "+$RETENTION_DAYS" -print0)
}

if [ "$RETENTION_DAYS" -ge 0 ]; then
    prune_cache
fi

exit "$failed"
