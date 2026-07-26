#!/bin/bash
set -euo pipefail

# Pull-based updater for hosts that cannot build (or even evaluate) their own
# configuration. A builder publishes a manifest naming the store path of this
# host's current system closure; we fetch it, substitute the closure from the
# builder's binary cache, and activate it. No git checkout, no nix evaluation,
# no compilation — the peak memory cost here is `nix copy`.
#
# Counterpart to services.bcnelson.closurePublisher.

tempfile=$(mktemp)
complete=0
rebootRequired=0

log() {
    echo "$1" |& tee -a "$tempfile"
}

if [ -z "${MANIFEST_URL-}" ]; then
    log "MANIFEST_URL not set"
    exit 1
fi

if [ -z "${CACHE_URL-}" ]; then
    log "CACHE_URL not set"
    exit 1
fi

if [ -n "${HEALTHCHECK_UUID_FILE-}" ] && [ -f "$HEALTHCHECK_UUID_FILE" ]; then
    HEALTHCHECK_UUID="$(cat "$HEALTHCHECK_UUID_FILE")"
else
    HEALTHCHECK_UUID=""
fi

if [ -n "${NTFY_TOPIC_FILE-}" ] && [ -f "$NTFY_TOPIC_FILE" ]; then
    NTFY_TOPIC="$(cat "$NTFY_TOPIC_FILE")"
else
    NTFY_TOPIC=""
fi

if [ -n "$HEALTHCHECK_UUID" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
    if ! curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/start"; then
        log "Failed to start healthcheck ping uuid: $HEALTHCHECK_UUID"
    fi
fi

cleanup_and_exit() {
    if [ -n "$HEALTHCHECK_UUID" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
        if [ "$complete" -eq 0 ]; then
            # Same reasoning as autoUpdate: send the log tail on stdin, because a
            # failure log can exceed the kernel's per-argument limit and would
            # then stop curl from exec'ing at all.
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

# Persistent retry state, mirroring autoUpdate: a closure that fails to
# activate must keep being retried (and keep reporting failure) rather than
# being silently skipped on the next tick.
STATE_DIR="${STATE_DIRECTORY:-/var/lib/remote-update}"
STATE_FILE="$STATE_DIR/state"
MAX_RETRIES="${MAX_RETRIES:-3}"

APPLIED_PATH=""
ATTEMPT_PATH=""
ATTEMPT_COUNT=0

mkdir -p "$STATE_DIR"
if [ -f "$STATE_FILE" ]; then
    # shellcheck disable=SC1090
    source "$STATE_FILE"
fi

save_state() {
    {
        echo "APPLIED_PATH=$APPLIED_PATH"
        echo "ATTEMPT_PATH=$ATTEMPT_PATH"
        echo "ATTEMPT_COUNT=$ATTEMPT_COUNT"
    } > "$STATE_FILE"
}

log "Fetching manifest from $MANIFEST_URL"
# The cache is served with `expires max`, and a stale manifest would pin this
# host to an old generation forever, so ask for a fresh copy explicitly.
if ! manifest=$(curl --silent --show-error --fail --location --retry 5 --retry-delay 2 \
        --header "Cache-Control: no-cache" "$MANIFEST_URL"); then
    log "Failed to fetch manifest from $MANIFEST_URL"
    cleanup_and_exit 1
fi

target=$(head -n1 <<<"$manifest" | tr -d '[:space:]')

# A missing manifest is answered by the cache's upstream fallback, so an
# unexpected body means "not published", not "here is a store path".
if ! [[ "$target" =~ ^/nix/store/[a-z0-9]{32}-.+$ ]]; then
    log "Manifest did not contain a store path (got: ${target:0:120})"
    cleanup_and_exit 1
fi

current=$(readlink -f /run/current-system)
if [ "$target" == "$current" ]; then
    log "Already running $target"
    APPLIED_PATH="$target"
    ATTEMPT_PATH=""
    ATTEMPT_COUNT=0
    save_state
    complete=1
    cleanup_and_exit 0
fi

if [ "$target" != "$ATTEMPT_PATH" ]; then
    ATTEMPT_PATH="$target"
    ATTEMPT_COUNT=0
fi

if [ "$ATTEMPT_COUNT" -ge "$MAX_RETRIES" ]; then
    log "$target failed $ATTEMPT_COUNT times; retry limit ($MAX_RETRIES) reached, not retrying until a new closure is published"
    save_state
    complete=0
    cleanup_and_exit 1
fi

# Count the attempt before making it, so a crash or timeout still counts.
ATTEMPT_COUNT=$((ATTEMPT_COUNT + 1))
log "Activation attempt $ATTEMPT_COUNT/$MAX_RETRIES for $target"
save_state

log "Copying closure from $CACHE_URL"
copyArgs=(--from "$CACHE_URL")
checkSignatures="${CHECK_SIGNATURES:-true}"
if [ "${checkSignatures,,}" != "true" ]; then
    # Scoped to this one fetch on purpose: leaving require-sigs on globally
    # keeps every other substitution on this host signature checked.
    copyArgs+=(--no-check-sigs)
fi
if ! nix copy "${copyArgs[@]}" "$target" |& tee -a "$tempfile"; then
    log "Failed to copy closure $target"
    cleanup_and_exit 1
fi

# Guard against a manifest mix-up handing this machine another host's system.
if [ -n "${EXPECTED_HOSTNAME-}" ] && [ -r "$target/etc/hostname" ]; then
    closureHostname=$(tr -d '[:space:]' < "$target/etc/hostname")
    if [ "$closureHostname" != "$EXPECTED_HOSTNAME" ]; then
        log "Refusing to activate: closure is for '$closureHostname', this host is '$EXPECTED_HOSTNAME'"
        cleanup_and_exit 1
    fi
fi

log "Setting system profile to $target"
if ! nix-env --profile /nix/var/nix/profiles/system --set "$target" |& tee -a "$tempfile"; then
    log "Failed to set system profile"
    cleanup_and_exit 1
fi

log "Activating $target"
if ! "$target/bin/switch-to-configuration" switch |& tee -a "$tempfile"; then
    log "switch failed; staging the closure for the next boot instead"
    if ! "$target/bin/switch-to-configuration" boot |& tee -a "$tempfile"; then
        log "Failed to stage closure for boot"
        cleanup_and_exit 1
    fi
    rebootRequired=1
fi

APPLIED_PATH="$target"
ATTEMPT_PATH=""
ATTEMPT_COUNT=0
save_state

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
    reboot="${REBOOT:-false}"
    if [[ "${reboot,,}" =~ ^(no|n|false|0)$ ]]; then
        log "Reboot skipped"
    else
        shutdown -r +1 "Rebooting for updates in 1 minute"
        if [ -n "$NTFY_TOPIC" ]; then
            curl --silent --show-error --retry 5 \
                -H "X-Title: $(hostname -s) rebooting in 1 min" \
                -d "$(hostname -s) is rebooting in 1 min as necessary for updates" \
                "https://ntfy.sh/$NTFY_TOPIC" || true
        fi
        if [ -n "$HEALTHCHECK_UUID" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/log" \
                --data-raw "Rebooting for updates in 1 minute" || true
        fi
    fi
fi

complete=1
