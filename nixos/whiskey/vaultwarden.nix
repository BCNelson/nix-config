{ config, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };
in
{
  services.nginx = {
    enable = true;
    virtualHosts = {
      "vault.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 525M;
        '';
        locations = {
          "/" = {
            proxyWebsockets = true;
            proxyPass = "http://localhost:8080";
          };
          "/admin" = {
            extraConfig = ''
                allow 100.64.0.0/10;
                deny all;
            '';
            proxyPass = "http://localhost:8080";
          };
        };
      };
    };
  };

  age.secrets.vaultwarden.rekeyFile = ../../secrets/store/vaultwarden.age;

  services.vaultwarden = {
    enable = true;
    dbBackend = "sqlite";
    backupDir = "/data/level1/vaultwarden";
    config = {
        WEBSOCKET_ENABLED = true;
        LOG_LEVEL = "info";
        SIGNUPS_ALLOWED = false;
        INVITATIONS_ALLOWED = true;
        INVITATION_ORG_NAME = "Nelson Family";
        DOMAIN = "https://vault.nel.family";
        SMTP_HOST = "smtp.migadu.com";
        SMTP_FROM = "admin@nel.family";
        SMTP_FROM_NAME = "VaultWarden";
        SMTP_PORT = 465;
        SMTP_SSL = true;
        SMTP_EXPLICIT_TLS = true;
        SMTP_TIMEOUT = 15;
        HELO_NAME = "whiskey";
        SMTP_DEBUG = false;
        ROCKET_ADDRESS = "::1";
        ROCKET_PORT = 8080;
    };
    environmentFile = config.age.secrets.vaultwarden.path;
  };
}
