#!/usr/bin/env -S just --justfile

[private]
default:
  @just --list --justfile {{justfile()}} --list-heading $'Avalible Commands\n'

all: update-os update-home

update:
    nix flake update

alias home :=update-home
alias h :=update-home
update-home:
    home-manager switch --flake .#$USER@$HOSTNAME

alias os :=update-os
alias o :=update-os
update-os:
    sudo nixos-rebuild switch --flake .#$HOSTNAME

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

sync: pull home os


alias fmt :=format
format:
    nix fmt

iso:
    #!/usr/bin/env bash
    set -euxo pipefail
    nix build .#nixosConfigurations.iso_desktop.config.system.build.isoImage -o {{justfile_directory()}}/../result
    ISO=$(head -n1 {{justfile_directory()}}/../result/nix-support/hydra-build-products | cut -d'/' -f6)
    sudo cp {{justfile_directory()}}/../result/iso/$ISO {{justfile_directory()}}/qemu/desktop/desktop.iso

alias t :=test
[no-exit-message]
@test *recipe='list':
    just -f ./test/justfile {{recipe}}
