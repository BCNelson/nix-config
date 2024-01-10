{ dataDirs }:
{
  syncthing = {
    image = "ghcr.io/linuxserver/syncthing";
    container_name = "syncthing";
    environment = [
      "PUID=1000"
      "PGID=1000"
      "TZ=America/Denver"
    ];
    volumes = [
      "${dataDirs.level5}/syncthing/config:/config"
      "${dataDirs.level4}/syncthing/data:/data/folders/level4"
    ];
    ports = [
      "22000:22000/tcp"
      "22000:22000/udp"
      "21027:21027/udp"
    ];
    restart = "unless-stopped";
  };
}
