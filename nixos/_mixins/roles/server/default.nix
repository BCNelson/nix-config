{ pkgs, ... }:
{
    imports = [
        ../docker.nix
    ];

    services.openssh.enable = true;
    # Add nix-ld so that we can use vscode remote ssh
    programs.nix-ld.enable = true;
}