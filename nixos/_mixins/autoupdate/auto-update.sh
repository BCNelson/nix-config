#!/bin/bash
tempfile=$(mktemp)
complete=0

log() {
    echo "$1" | tee -a "$tempfile"
}

if ! curl "https://health.b.nel.family/ping/$HEALTHCHECK_UUID/start"; then
    log "Failed to start healthcheck ping uuid: $HEALTHCHECK_UUID"
fi

exit() {
    if [ "$complete" -eq 0 ]; then
        curl --retry 5 --data-raw "$(cat "$tempfile")" "https://health.b.nel.family/ping/$HEALTHCHECK_UUID/fail"
    else
        curl --retry 5 "https://health.b.nel.family/ping/$HEALTHCHECK_UUID"
    fi
    trap - EXIT
}

trap exit EXIT

cd /config || exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null;
then
    log "Locked and must be unlocked before update"
    fail
fi

hashBefore=$(git rev-parse HEAD)
git pull --rebase | tee -a "$tempfile"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    log "Failed to pull"
    exit
fi
hashAfter=$(git rev-parse HEAD)

if [ "$hashBefore" == "$hashAfter" ]; then
    log "No changes"
    complete=1
    exit
fi

nixos-rebuild switch --flake ".#$(hostname)" | tee -a "$tempfile"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    log "Failed to rebuild"
    exit
fi


# Check if a reboot is required

current_system_initrd=$(readlink /run/current-system/initrd)
current_system_kernel=$(readlink /run/current-system/kernel)
current_system_kernel_modules=$(readlink /run/current-system/kernel-modules)

booted_system_initrd=$(readlink /run/booted-system/initrd)
booted_system_kernel=$(readlink /run/booted-system/kernel)
booted_system_kernel_modules=$(readlink /run/booted-system/kernel-modules)


if [ "$current_system_initrd" != "$booted_system_initrd" ] ||
   [ "$current_system_kernel" != "$booted_system_kernel" ] ||
   [ "$current_system_kernel_modules" != "$booted_system_kernel_modules" ];
then
    log "Reboot required"
    shutdown -r +1 "Rebooting for updates in 1 minute"
    # check if NTFY_TOPIC is set
    if [ -n "$NTFY_TOPIC" ]; then
        log "Sending notification to https://ntfy.sh/$NTFY_TOPIC"
        curl -H "X-Title: $(hostname) rebooting in 1 min" \
          -d "$(hostname) is rebooting in 1 min as necessary for updates" \
          "https://ntfy.sh/$NTFY_TOPIC"
        curl --retry 5 "https://health.b.nel.family/ping/$HEALTHCHECK_UUID/log" \
          --data-raw "Rebooting for updates in 1 minute"
    fi
fi

complete=1