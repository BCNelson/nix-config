{ dataDirs, porkbun }:
let
  config = ".";
in
{
  swag = {
    build = "./swag";
    container_name = "swag";
    cap_add = [ "NET_ADMIN" ];
    environment = [
      "PUID=1002"
      "PGID=1002"
      "TZ=America/Denver"
      "URL=h.b.nel.family"
      "SUBDOMAINS=wildcard"
      "VALIDATION=dns"
      "DNSPLUGIN=porkbun"
      "EMAIL=bradley@nel.family"
      "DHLEVEL=2048"
      "ONLY_SUBDOMAINS=true"
      "STAGING=false"
      "EXTRA_DOMAINS= *.romeo.b.nel.family, *.nel.family *.bnel.me nel.to"
      "DOCKER_MODS=linuxserver/mods:swag-auto-reload"
    ];
    volumes = [
      "${dataDirs.level7}/swag:/config"
      "${porkbun}/porkbun.ini:/config/dns-conf/porkbun.ini:ro"
      "${config}/swag/nginx/proxy-confs:/config/nginx/proxy-confs:ro"
      "${config}/swag/nginx/tailscale.conf:/config/nginx/tailscale.conf:ro"
      "${config}/swag/nginx/internal.conf:/config/nginx/internal.conf:ro"
    ];
    ports = [
      "443:443"
      "80:80"
    ];
    restart = "unless-stopped";
    # networks = [ "external" ]; # TODO: figure out how to make network definion cleaner
  };
}
