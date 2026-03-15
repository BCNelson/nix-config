{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  virtualisation.oci-containers.containers.syncthing = {
    image = "ghcr.io/linuxserver/syncthing";
    ports = [
      "22000:22000/tcp"
      "22000:22000/udp"
      "21027:21027/udp"
      "8384:8384"
    ];
    volumes = [
      "${dataDirs.level5}/syncthing/config:/config"
      "${dataDirs.level4}/syncthing/data:/data/folders/level4"
    ];
    environment = {
      PUID = "1000";
      PGID = "1000";
      TZ = "America/Denver";
    };
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
