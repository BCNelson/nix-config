{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  age.secrets.jellyswarrm-password = {
    rekeyFile = ./secrets/jellyswarrm_password.age;
    generator.script = "passphrase";
    mode = "0400";
    owner = "jellyswarrm";
    bitwarden = {
      name = "Jellyswarrm Admin Password";
      username = "admin";
      uris = { uri = "https://jellyswarrm.h.b.nel.family"; matchType = "host"; };
    };
  };

  services.jellyswarrm = {
    enable = true;
    host = "127.0.0.1";
    port = 3001;
    dataDir = "${dataDirs.level5}/jellyswarrm";
    passwordFile = config.age.secrets.jellyswarrm-password.path;
  };

  services.nginx.virtualHosts."jellyswarrm.h.b.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:3001";
      proxyWebsockets = true;
    };
  };
}
