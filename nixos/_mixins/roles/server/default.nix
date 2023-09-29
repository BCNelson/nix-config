{ pkgs, ... }:
{
    imports = [
        ../docker.nix
    ];

    environment.systemPackages = [
        pkgs.tmux
    ];
}