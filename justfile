#!/usr/bin/env -S just --justfile

# mod test

[private]
default:
  @just --list --justfile {{justfile()}} --list-heading $'Avalible Commands\n'

all: update-os update-home

update:
    nix flake update

alias home :=update-home
alias h :=update-home
[linux]
update-home *additionalArgs:
    home-manager switch --flake .#$USER@$HOSTNAME {{additionalArgs}}

[macos]
update-home *additionalArgs:
    home-manager switch --flake .#$USER@$(hostname -s) {{additionalArgs}}

alias apply :=update-os
alias os :=update-os
alias o :=update-os
[linux]
update-os *additionalArgs:
    #!/usr/bin/env bash
    if [ "$EUID" -ne 0 ]
    then
        sudo nixos-rebuild switch --flake .#$HOSTNAME {{additionalArgs}}
    else
        nixos-rebuild switch --flake .#$HOSTNAME {{additionalArgs}}
    fi

    
[macos]
update-os *additionalArgs:
    #!/usr/bin/env zsh
    confilictingFile=("/etc/bashrc" "/etc/zshrc")
    for file in $confilictingFile
    do
        if [ -f $file ] && [ ! -L $file ];
        then
            echo "Moving $file to $file.$(date +%s).bak"
            sudo mv $file $file.$(date +%s).bak
        fi
    done
    /run/current-system/sw/bin/darwin-rebuild switch --flake {{justfile_directory()}} {{additionalArgs}}

[unix]
lock:
    git-crypt lock

[unix]
unlock:
    #!/usr/bin/env bash
    # set -euxo pipefail
    git config --local --get filter.git-crypt.smudge > /dev/null
    # check if the lat command was successful
    if [ $? -ne 0 ]; then
        # check if there are changes that need to be stashed
        if [ -n "$(git status --porcelain)" ]; then
            echo "Stashing"
            git stash push -k
            STASHED=true
        fi

        echo "Unlocking"
        gpg --pinentry-mode ask --decrypt local.key.asc | git-crypt unlock -

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

[linux]
check *additionalArgs:
    nix flake check {{additionalArgs}}

alias fmt :=format
[unix]
format:
    nix fmt

iso:
    #!/usr/bin/env bash
    set -euo pipefail
    nix build .#nixosConfigurations.iso_desktop.config.system.build.isoImage -o {{justfile_directory()}}/../result
    ISO=$(head -n1 {{justfile_directory()}}/../result/nix-support/hydra-build-products | cut -d'/' -f6)
    if test -e /dev/disk/by-label/ventoy; then
        ventoy_Mount=$(findmnt -n -o TARGET /dev/disk/by-label/ventoy)
        if [ -n "$ventoy_Mount" ]; then
            echo "Copying iso to ventoy drive"
            cp {{justfile_directory()}}/../result/iso/$ISO $ventoy_Mount/Nixos_Install_Desktop.iso
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
    nix run nix-darwin -- switch --flake {{justfile_directory()}}

build machine='vm_test' type='vm':
    nix build .#nixosConfigurations.{{machine}}.config.system.build.{{type}} -o {{justfile_directory()}}/result