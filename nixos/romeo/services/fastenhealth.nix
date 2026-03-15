{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  virtualisation.oci-containers.containers.fastenhealth = {
    image = "ghcr.io/fastenhealth/fasten-onprem:main";
    ports = [ "127.0.0.1:8081:8080" ];
    volumes = [
      "${dataDirs.level3}/fastenhealth/db:/opt/fasten/db"
      "${dataDirs.level7}/fastenhealth/cache:/opt/fasten/cache"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
