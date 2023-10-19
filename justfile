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
[linux]
update-os:
    sudo nixos-rebuild switch --flake .#$HOSTNAME
[macos]
update-os:
    darwin-rebuild switch --flake .

[unix]
unlock:
    gpg --decrypt local.key.asc | git-crypt unlock -

alias fmt :=format
[unix]
format:
    nix fmt

alias t :=test
[no-exit-message]
@test *recipe='list':
    just -f ./test/justfile {{recipe}}

[macos]
setup:
    nix run nix-darwin -- switch --flake .
