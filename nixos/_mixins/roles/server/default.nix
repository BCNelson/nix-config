{ pkgs, ... }:
{
    imports = [
        ../docker.nix
    ];

    services.openssh.enable = true;
}