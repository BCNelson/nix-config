{ pkgs, ... }:
{
    imports = [
        ../dokcer.nix
    ];

    environment.systemPackages = [
        pkgs.tmux
    ];
}