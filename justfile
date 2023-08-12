#!/usr/bin/env -S just --justfile

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

alias t :=test
test recipe='list':
    @just -f ./tests/justfile {{recipe}}

isoDesktop:
    nix build .#nixosConfigurations.iso_desktop.config.system.build.isoImage
