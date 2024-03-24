#!/bin/sh

curl "https://health.b.nel.family/ping/$HEALTHCHECK_UUID/start"

fail() {
    curl "https://health.b.nel.family/ping/$HEALTHCHECK_UUID/fail"
}

trap fail INT

cd /config || exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null;
then
    echo "Locked and must be unlocked before update"
    exit 1
fi

just --unstable sync

# Fail if sync returns an error
if [ $? -ne 0 ]; then
    fail
    exit 1
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
    echo "Reboot required"
    shutdown -r +1 "Rebooting for updates in 1 minute"
    # check if NTFY_TOPIC is set
    if [ -n "$NTFY_TOPIC" ]; then
        echo "Sending notification to https://ntfy.sh/$NTFY_TOPIC"
        curl -H "X-Title: $(hostname) rebooting in 2 mins" \
          -d "$(hostname) is rebooting in 1 min as necessary for updates" \
          "https://ntfy.sh/$NTFY_TOPIC"
    fi
fi

curl "https://health.b.nel.family/ping/$HEALTHCHECK_UUID"