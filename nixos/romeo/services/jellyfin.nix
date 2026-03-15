{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  virtualisation.oci-containers.containers.jellyfin = {
    image = "docker.io/jellyfin/jellyfin";
    user = "1000:1000";
    ports = [ "127.0.0.1:8096:8096" ];
    volumes = [
      "${dataDirs.level5}/jellyfin/config:/config"
      "${dataDirs.level7}/jellyfin/cache:/cache"
      "${dataDirs.level6}/media:/media:ro"
    ];
    environment = {
      JELLYFIN_PublishedServerUrl = "https://media.nel.family";
    };
    extraOptions = [
      "--device=/dev/dri:/dev/dri"
      "--group-add=303"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
