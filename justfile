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
    gpg --decrypt local.key.asc | git-crypt unlock -

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
