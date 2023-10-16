{ ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
    ../_mixins/roles/gaming.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/desktop
  ];
}
