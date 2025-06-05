{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  virtualisation.oci-containers.containers.node-red = {
    image = "ghcr.io/node-red/node-red:latest-debian";
    environment = {
      "TZ" = "America/Denver";
      "NODE_RED_ENABLE_PROJECTS" = "true";
    };
    volumes = [
      "${dataDirs.level5}/node-red/data:/data"
    ];
    ports = [ "127.0.0.1:1880:1880" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
  services.nginx = {
    enable = true;
    virtualHosts = {
      "nodered.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:1880";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
    };
  };
}