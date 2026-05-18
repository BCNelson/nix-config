#!/usr/bin/env -S just --justfile
# mod test

[private]
default:
    @just --list --justfile {{ justfile() }} --list-heading $'Avalible Commands\n'

all: update-os

update:
    nix flake update

alias apply := update-os
alias os := update-os
alias o := update-os

alias home := update-home
alias h := update-home

[unix]
update-home *additionalArgs:
    home-manager switch --flake .#bcnelson@$HOSTNAME {{ additionalArgs }}

[linux]
update-os *additionalArgs:
    #!/usr/bin/env bash
    set -euo pipefail
    git rev-parse HEAD
    if nix eval .#nixosConfigurations.$HOSTNAME --apply 'x: ""' --raw >/dev/null 2>&1; then
        if [ "$EUID" -ne 0 ]; then
            sudo nixos-rebuild switch --flake .#$HOSTNAME {{ additionalArgs }}
        else
            nixos-rebuild switch --flake .#$HOSTNAME {{ additionalArgs }}
        fi
    elif nix eval .#systemConfigs.$HOSTNAME --apply 'x: ""' --raw >/dev/null 2>&1; then
        nix run github:numtide/system-manager -- switch --flake .#$HOSTNAME --sudo {{ additionalArgs }}
    else
        echo "No nixosConfigurations or systemConfigs entry for host $HOSTNAME" >&2
        exit 1
    fi

[linux]
rollback:
    #!/usr/bin/env bash
    set -euo pipefail
    GEN=$(nixos-rebuild list-generations | fzf --tac --header="Select generation to switch to")
    if [ -n "$GEN" ]; then
        GEN_NUM=$(echo "$GEN" | awk '{print $1}')
        echo "Switching to generation $GEN_NUM..."
        if [ "$EUID" -ne 0 ]; then
            sudo /nix/var/nix/profiles/system-${GEN_NUM}-link/bin/switch-to-configuration switch
        else
            /nix/var/nix/profiles/system-${GEN_NUM}-link/bin/switch-to-configuration switch
        fi
    fi

[macos]
update-os *additionalArgs:
    #!/usr/bin/env zsh
    git rev-parse HEAD
    confilictingFile=("/etc/bashrc" "/etc/zshrc")
    for file in $confilictingFile
    do
        if [ -f $file ] && [ ! -L $file ];
        then
            echo "Moving $file to $file.$(date +%s).bak"
            sudo mv $file $file.$(date +%s).bak
        fi
    done
    /run/current-system/sw/bin/darwin-rebuild switch --flake {{ justfile_directory() }} {{ additionalArgs }}

[unix]
lock:
    git-crypt lock

[unix]
unlock:
    #!/usr/bin/env bash
    # set -euxo pipefail
    git config --local --get filter.git-crypt.smudge > /dev/null
    # check if the last command was successful
    if [ $? -ne 0 ]; then
        # check if there are changes that need to be stashed
        if [ -n "$(git status --porcelain)" ]; then
            echo "Stashing"
            git stash push -k
            STASHED=true
        fi

        echo "Unlocking"
        gpg --decrypt local.key.asc | git-crypt unlock -

        # check if there were changes that were stashed
        if [ "$STASHED" = true ]; then
            echo "Popping stash"
            git stash pop
        fi
    else
        echo "Already unlocked"
    fi

pull: unlock
    #!/usr/bin/env bash
    export GH_TOKEN=$(nix eval --file ./nixos/sensitive.nix gh_token | tail -c +2 | head -c -2)
    git -c credential.helper='!f() { sleep 1; echo "username=${GIT_USER}"; echo "password=${GH_TOKEN}"; }; f' pull --rebase

push: unlock
    #!/usr/bin/env bash
    export GIT_USER="bcnelson"
    export GH_TOKEN=$(nix eval --file ./nixos/sensitive.nix gh_token | tail -c +2 | head -c -2)
    git -c credential.helper='!f() { sleep 1; echo "username=${GIT_USER}"; echo "password=${GH_TOKEN}"; }; f' push

sync: pull update-os

alias check := check-host

[linux]
check-host host *additionalArgs:
    nix build .#nixosConfigurations.{{ host }}.config.system.build.toplevel --dry-run {{ additionalArgs }}

alias fmt := format

[unix]
format:
    nix fmt

# One-time bootstrap for SELinux-enforcing hosts (Fedora etc.). Installs the
# nix_store SELinux policy module, registers the file context rule, relabels
# the existing /nix/store (slow), and writes a sentinel so future `just apply`
# runs can refresh the module automatically.
[linux]
bootstrap-selinux:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v getenforce >/dev/null 2>&1 || [ "$(getenforce)" != "Enforcing" ]; then
        echo "SELinux is not enforcing on this host; nothing to do."
        exit 0
    fi
    pp=$(nix build --no-link --print-out-paths .#nix-store-selinux)/nix_store.pp
    sha=$(sha256sum "$pp" | cut -d' ' -f1)
    sudo install -d -m 0755 /var/lib/system-manager
    sudo semodule -i "$pp"
    if ! sudo semanage fcontext -l | grep -q '^/nix/store'; then
        sudo semanage fcontext -a -t nix_store_t '/nix/store(/.*)?'
    fi
    sudo -v   # prime sudo so the prompt doesn't break the progress line
    echo "Counting /nix/store entries..."
    total=$(find /nix/store -mindepth 1 | wc -l)
    echo "Relabeling $total entries..."
    # restorecon -p prints one '*' (or '.') per 1000 files. stdbuf disables
    # libc buffering on both streams so each char arrives immediately; bash
    # read -N1 consumes one byte at a time so there's no awk/pipe buffering
    # in between.
    sudo stdbuf -o0 -e0 restorecon -R -p /nix/store 2>&1 | (
      count=0
      while IFS= read -rN1 c; do
        case "$c" in
          '*'|'.')
            count=$((count + 1000))
            [ "$count" -gt "$total" ] && count=$total
            printf "\r[%3d%%] %d/%d" "$((count*100/total))" "$count" "$total"
            ;;
        esac
      done
      printf "\r[100%%] %d/%d (done)\n" "$total" "$total"
    )
    echo "$sha" | sudo tee /var/lib/system-manager/nix-store-selinux.sha256 >/dev/null
    sudo touch /var/lib/system-manager/nix-store-selinux-bootstrapped
    echo "nix_store SELinux policy bootstrapped."

isoCreate version='iso_desktop':
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s extglob
    nix build .#nixosConfigurations.{{version}}.config.system.build.isoImage -o {{ justfile_directory() }}/result

isoTest version='iso_desktop': (isoCreate version)
    #!/usr/bin/env bash
    DISK_IMAGE={{ justfile_directory() }}/test/working/{{version}}/iso.qcow2
    mkdir -p $(dirname $DISK_IMAGE)
    if test -n $DISK_IMAGE && ! test -e $DISK_IMAGE; then
        mkdir -p {{ justfile_directory() }}/test_vm
        qemu-img create -f qcow2 "$DISK_IMAGE" "32G"
    fi
    ISO=$(head -n1 {{ justfile_directory() }}/result/nix-support/hydra-build-products | cut -d'/' -f6)
    qemu-system-x86_64-uefi -enable-kvm -m 8192 -cdrom {{ justfile_directory() }}/result/iso/$ISO -drive cache=writeback,file="$DISK_IMAGE",format=qcow2,media=disk

isoInstall version='iso_desktop': (isoCreate version)
    #!/usr/bin/env bash
    shopt -s extglob
    ISO_PATH=$(awk '{print $3}' "{{justfile_directory()}}/result/nix-support/hydra-build-products")
    if test -e /dev/disk/by-label/@(v|V)entoy; then
        ventoy_Mount=$(findmnt -n -o TARGET /dev/disk/by-label/@(v|V)entoy)
        if [ -n "$ventoy_Mount" ]; then
            echo "Copying iso to ventoy drive"

            cp "$ISO_PATH" $ventoy_Mount/Nixos_{{version}}.iso
        else
            echo "Ventoy drive not mounted"
        fi
    else
        echo "No ventoy drive found"
    fi

[macos]
setup:
    #!/usr/bin/env bash
    sudo mv /etc/bashrc /etc/bashrc.$(date +%s).bak
    sudo mv /etc/zshrc /etc/zshrc.$(date +%s).bak
    nix run nix-darwin -- switch --flake {{ justfile_directory() }}

build machine='vm_test' type='vm':
    nix build .#nixosConfigurations.{{ machine }}.config.system.build.{{ type }} -o {{ justfile_directory() }}/result

test machine='vm_test' type='vm':
    nix build .#nixosConfigurations.{{ machine }}.config.system.build.{{ type }} -o {{ justfile_directory() }}/result
    mkdir -p {{ justfile_directory() }}/test/working/test_vm
    NIX_DISK_IMAGE={{ justfile_directory() }}/test/working/test_vm/{{ machine }}.qcow2 ./result/bin/run-{{ machine }}-{{ type }}
    rm -rf {{ justfile_directory() }}/test/working/test_vm

rekey:
    agenix rekey -a

generate-secrets:
    @echo "🔐 Generating age secrets..."
    agenix generate -a

# Sync age secrets to Bitwarden
sync-secrets:
    @echo "🔐 Syncing age secrets to Bitwarden..."
    just unlock
    nix run .#age-bitwarden-sync -- --fido

terraform *args:
    #!/usr/bin/env bash
    export AWS_ACCESS_KEY_ID=$(nix eval --file ./nixos/sensitive.nix B2_TERRAFORM_STATE_KEY_ID | tail -c +2 | head -c -2)
    export AWS_SECRET_ACCESS_KEY=$(nix eval --file ./nixos/sensitive.nix B2_TERRAFORM_STATE_APPLICATION_KEY | tail -c +2 | head -c -2)
    terraform {{ args }}
