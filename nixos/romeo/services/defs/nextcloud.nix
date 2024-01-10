{ dataDirs }:
let
  sensitiveData = import ../../../sensitive.nix;
in
{
  nextcloud = {
    image = "ghcr.io/linuxserver/nextcloud:latest";
    container_name = "nextcloud";
    environment = [
      "PUID=1000"
      "PGID=1000"
      "TZ=America/Denver"
    ];
    volumes = [
      "${dataDirs.level5}/nextcloud/config:/config"
      "${dataDirs.level5}/nextcloud/data:/data"
    ];
    links = [ "nextcloud_db:db" ];
    restart = "unless-stopped";
  };
  nextcloud_db = {
    image = "postgres:12";
    restart = "unless-stopped";
    container_name = "nextcloud_db";
    volumes = [
      "${dataDirs.level5}/nextcloud/database:/var/lib/postgresql/data"
    ];
    environment = [
      "POSTGRES_PASSWORD=${sensitiveData.nextcloud_postgress_password}"
    ];
  };
}
