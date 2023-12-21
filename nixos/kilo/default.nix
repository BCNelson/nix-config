{ inputs, pkgs, ... }:
{
  imports = [
    inputs.nixos-generators.nixosModules.linode
    ./hardware-configuration.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/server
    ../_mixins/users/bcnelson
  ];

  environment.systemPackages = with pkgs; [
    inetutils
    mtr
    sysstat
  ];
}
