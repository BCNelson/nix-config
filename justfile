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

test-deckmaster:
    #!/usr/bin/env bash
    cleanup() {
        systemctl --user start deckmaster.path
        trap - SIGINT SIGTERM # clear the trap
        kill -- -$$ # Sends SIGTERM to child/sub processes
    }
    systemctl --user stop deckmaster.path
    systemctl --user stop deckmaster.service
    deckmaster -deck ./home-manager/deckmaster/files/main.deck
    
    trap cleanup EXIT
