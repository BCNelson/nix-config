{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  imports = [
    ../_mixins/roles/nfs.nix
  ];

  fileSystems."/export/photos" = {
    device = "${dataDirs.level2}/photos";
    options = [ "bind" "x-systemd.requires=zfs-import.target" ];
  };

  services.nfs.server = {
    enable = true;
    exports = ''
      /export/photos 100.64.0.0/10(rw,async)
    '';
  };

  users.groups = {
    photos = {
      name = "photos";
      gid = 27000;
      members = [ "bcnelson" "hlnelson" ];
    };
  };

  networking.firewall.allowedTCPPorts = [ 2049 ];
}
