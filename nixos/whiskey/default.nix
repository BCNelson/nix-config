args@{ pkgs, libx, ... }:
let
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
    ];
  networking.hostId = "9a637b7f";
}
