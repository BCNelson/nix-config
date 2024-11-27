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
      "health.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        locations = {
          "/" = {
            proxyPass = "http://localhost:8000";
          };
        };
      };
    };
  };

  age.secrets.healthchecks.rekeyFile = ../../secrets/store/healthchecks.age;

  services.healthchecks = {
    enable = true;
    dataDir = "${dataDirs.level3}/healthchecks";
    settings = {
        ALLOWED_HOSTS = [ "https://health.b.nel.family" ];
        SITE_ROOT = "https://health.b.nel.family";
        REGISTRATION_OPEN = false;
        DEBUG = true;
    };
    settingsFile = config.age.secrets.healthchecks.path;
    listenAddress = "localhost";
    port = 8000;
  };
}
