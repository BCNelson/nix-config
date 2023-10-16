{ ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
  ];
}
