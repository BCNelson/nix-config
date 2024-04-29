{ dataDirs, porkbunToken }:
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
      "URL=nel.family"
      "SUBDOMAINS=wildcard"
      "VALIDATION=dns"
      "DNSPLUGIN=porkbun"
      "EMAIL=bradley@nel.family"
      "DHLEVEL=2048"
      "ONLY_SUBDOMAINS=true"
      "STAGING=true"
      "EXTRA_DOMAINS=health.b.nel.family"
      "DOCKER_MODS=linuxserver/mods:swag-auto-reload"
    ];
    volumes = [
      "${dataDirs.level7}/swag:/config"
      "${porkbunToken}/porkbun.ini:/config/dns-conf/porkbun.ini:ro"
      "${config}/swag/nginx/proxy-confs:/config/nginx/proxy-confs:ro"
      "${config}/swag/nginx/tailscale.conf:/config/nginx/tailscale.conf:ro"
      "${config}/swag/nginx/internal.conf:/config/nginx/internal.conf:ro"
    ];
    ports = [
      "443:443"
      "80:80"
    ];
    restart = "unless-stopped";
  };
}
