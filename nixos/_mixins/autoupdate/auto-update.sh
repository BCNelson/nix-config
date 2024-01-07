#!/bin/sh

cd /config || exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null;
then
    echo "Locked and must be unlocked before update"
    exit 1
fi

just sync