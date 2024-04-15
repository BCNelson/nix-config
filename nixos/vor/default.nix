args@{ libx, ... }:
let
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      (import ../_mixins/autoupdate (args // { inherit healthcheckUuid; }))
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ./samba.nix
      ./backups.nix
    ];
  networking.hostId = "d80836c3";
}
