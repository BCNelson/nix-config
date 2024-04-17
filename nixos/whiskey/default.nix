args@{ libx, pkgs, ... }:
let
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      (import ../_mixins/autoupdate (args // { inherit pkgs healthcheckUuid; }))
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
    ];
  networking.hostId = "9a637b7f";
}
