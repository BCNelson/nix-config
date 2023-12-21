{ inputs, pkgs, ... }:
let
  linodeImage = inputs.nixpkgs + "/nixos/modules/virtualisation/linode-image.nix";
in
{
  imports = [
    linodeImage
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
