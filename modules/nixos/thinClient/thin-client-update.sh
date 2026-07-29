#!/bin/bash
set -euo pipefail

# Pull-only system update for hosts that cannot evaluate their own closure.
#
# This deliberately mirrors auto-update.sh (health check pings, per-target retry
# state, reboot detection) but replaces "git pull + nixos-rebuild" with "read a
# manifest + nix copy a prebuilt store path". Nothing here evaluates nix code or
# needs the flake checked out, so it runs fine in ~2 GB of RAM.

tempfile=$(mktemp)
chmod 777 "$tempfile"
complete=0

log() {
    echo "$1" |& tee -a "$tempfile"
}

for var in MANIFEST_URL CACHE_URL; do
    if [ -z "${!var-}" ]; then
        log "$var not set"
        exit 1
    fi
done

# Handle healthcheck file
if [ -n "${HEALTHCHECK_UUID_FILE-}" ] && [ -f "$HEALTHCHECK_UUID_FILE" ]; then
    HEALTHCHECK_UUID="$(cat "$HEALTHCHECK_UUID_FILE")"
else
    HEALTHCHECK_UUID=""
fi

if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
    if ! curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/start"; then
        log "Failed to start healthcheck ping uuid: $HEALTHCHECK_UUID"
    fi
fi

cleanup_and_exit() {
    if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
        if [ "$complete" -eq 0 ]; then
            # Send the log as the request body via stdin, not as a CLI argument
            # -- see the same note in auto-update.sh. Cadence keeps the head of
            # the body, so send the tail: the real error is at the end.
            tail -c 10000 "$tempfile" | curl --silent --show-error --retry 5 \
                --data-binary @- "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/fail" || true
        else
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID" || true
        fi
    fi
    rm -f "$tempfile"
    trap - EXIT
    exit "${1:-0}"
}

trap 'cleanup_and_exit $?' EXIT

# Persistent retry state across timer-spaced runs, keyed on the store path we
# were asked to switch to rather than on a commit. Same reasoning as
# auto-update.sh: without this a closure that fails to activate is attempted
# exactly once, and every later run reports success because "nothing changed".
STATE_DIR="${STATE_DIRECTORY:-/var/lib/thin-client-update}"
STATE_FILE="$STATE_DIR/state"
MAX_RETRIES="${MAX_RETRIES:-3}"

SWITCHED_PATH=""
ATTEMPT_PATH=""
ATTEMPT_COUNT=0

mkdir -p "$STATE_DIR"
if [ -f "$STATE_FILE" ]; then
    # shellcheck disable=SC1090
    source "$STATE_FILE"
fi

save_state() {
    {
        echo "SWITCHED_PATH=$SWITCHED_PATH"
        echo "ATTEMPT_PATH=$ATTEMPT_PATH"
        echo "ATTEMPT_COUNT=$ATTEMPT_COUNT"
    } > "$STATE_FILE"
}

log "Fetching manifest $MANIFEST_URL"
manifest=$(curl --silent --show-error --fail --retry 3 --retry-delay 5 --max-time 60 "$MANIFEST_URL" 2>>"$tempfile") || {
    log "Failed to fetch manifest from $MANIFEST_URL"
    cleanup_and_exit 1
}

status=$(jq -r '.status // "unknown"' <<<"$manifest")
storePath=$(jq -r '.storePath // ""' <<<"$manifest")
commit=$(jq -r '.commit // "unknown"' <<<"$manifest")

if [ "$status" != "ready" ]; then
    # The builder publishes status=failed with the error it hit. Surface that
    # here rather than sitting silently on a stale closure -- otherwise a host
    # whose build has been broken for a week looks perfectly healthy.
    log "Manifest status is '$status' (commit $commit), not 'ready'"
    error=$(jq -r '.error // ""' <<<"$manifest")
    [ -n "$error" ] && log "Builder reported: $error"
    cleanup_and_exit 1
fi

if [ -z "$storePath" ] || [[ "$storePath" != /nix/store/* ]]; then
    log "Manifest did not contain a usable storePath: '$storePath'"
    cleanup_and_exit 1
fi

log "Manifest: commit $commit -> $storePath"

currentSystem=$(readlink -f /run/current-system)
if [ "$storePath" == "$currentSystem" ]; then
    log "Already running $storePath; up to date"
    complete=1
    cleanup_and_exit 0
fi

if [ "$storePath" == "$SWITCHED_PATH" ]; then
    # We switched to this closure but /run/current-system does not point at it,
    # which means the switch was staged for boot. Nothing more to do until the
    # reboot happens.
    log "Already switched to $storePath; waiting on reboot to take effect"
    complete=1
    cleanup_and_exit 0
fi

if [ "$storePath" != "$ATTEMPT_PATH" ]; then
    ATTEMPT_PATH="$storePath"
    ATTEMPT_COUNT=0
fi

if [ "$ATTEMPT_COUNT" -ge "$MAX_RETRIES" ]; then
    log "Closure $storePath failed $ATTEMPT_COUNT times; retry limit ($MAX_RETRIES) reached, not retrying"
    save_state
    complete=0
    cleanup_and_exit 1
fi

# Persist the incremented count BEFORE doing the work so a timeout or a hard
# crash still counts as an attempt.
ATTEMPT_COUNT=$((ATTEMPT_COUNT + 1))
log "Update attempt $ATTEMPT_COUNT/$MAX_RETRIES for $storePath"
save_state

# Fetch the closure. Signatures are checked normally: the builder signs with the
# cache key whose public half is in nix.settings.trusted-public-keys, and the
# parts of the closure that came from upstream still carry cache.nixos.org's
# signature.
log "Copying closure from $CACHE_URL"
if ! nix copy --extra-experimental-features "nix-command flakes" \
        --from "$CACHE_URL" "$storePath" |& tee -a "$tempfile"; then
    log "Failed to copy $storePath from $CACHE_URL"
    cleanup_and_exit 1
fi

if [ ! -e "$storePath/bin/switch-to-configuration" ]; then
    log "$storePath does not look like a system closure (no bin/switch-to-configuration)"
    cleanup_and_exit 1
fi

# Register the closure as the system profile before activating, so a failed
# switch still leaves a bootable generation in the boot menu.
if ! nix-env --profile /nix/var/nix/profiles/system --set "$storePath" |& tee -a "$tempfile"; then
    log "Failed to set the system profile to $storePath"
    cleanup_and_exit 1
fi

# Activate. If `switch` is refused by switchInhibitors (dbus or kernel changes
# that cannot be applied live) fall back to `boot` and schedule a reboot, so the
# generation is not left staged forever -- same fallback as auto-update.sh.
switch_output=$(mktemp)
rebootRequired=0
if ! "$storePath/bin/switch-to-configuration" switch |& tee -a "$tempfile" "$switch_output"; then
    if grep -qF "Pre-switch check 'switchInhibitors' failed" "$switch_output"; then
        log "switch blocked by switchInhibitors; falling back to 'boot' and scheduling reboot"
        if ! "$storePath/bin/switch-to-configuration" boot |& tee -a "$tempfile"; then
            log "Failed to activate (boot fallback)"
            rm -f "$switch_output"
            cleanup_and_exit 1
        fi
        rebootRequired=1
    else
        log "Failed to activate $storePath"
        rm -f "$switch_output"
        cleanup_and_exit 1
    fi
fi
rm -f "$switch_output"

SWITCHED_PATH="$storePath"
ATTEMPT_PATH=""
ATTEMPT_COUNT=0
save_state

# Keep a GC root so a `nix-collect-garbage` between the switch and the reboot
# cannot remove the closure we are about to boot into.
mkdir -p /nix/var/nix/gcroots
ln -sfn "$storePath" /nix/var/nix/gcroots/thin-client-current

current_system_initrd=$(readlink /run/current-system/initrd)
current_system_kernel=$(readlink /run/current-system/kernel)
current_system_kernel_modules=$(readlink /run/current-system/kernel-modules)

booted_system_initrd=$(readlink /run/booted-system/initrd)
booted_system_kernel=$(readlink /run/booted-system/kernel)
booted_system_kernel_modules=$(readlink /run/booted-system/kernel-modules)

if [ "$rebootRequired" -eq 1 ] ||
   [ "$current_system_initrd" != "$booted_system_initrd" ] ||
   [ "$current_system_kernel" != "$booted_system_kernel" ] ||
   [ "$current_system_kernel_modules" != "$booted_system_kernel_modules" ]; then
    log "Reboot required"
    if [[ "${REBOOT,,}" =~ ^(no|n|false|0)$ ]]; then
        log "Reboot skipped"
    else
        shutdown -r +1 "Rebooting for updates in 1 minute"
        if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/log" \
                --data-raw "Rebooting for updates in 1 minute" || true
        fi
    fi
fi

log "Updated to $storePath (commit $commit)"
complete=1
