{ ... }:
{
  imports = [
    ../docker.nix
  ];

  services.openssh = {
    enable = true;
    openFirewall = true;
  };
  # Add nix-ld so that we can use vscode remote ssh
  programs.nix-ld.enable = true;

  # Passwordless sudo for wheel-group users on servers.
  security.sudo.wheelNeedsPassword = false;
}
