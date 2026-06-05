{ config, inputs, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };
in
{
  imports = [ inputs.cadence.nixosModules.cadence ];

  age.secrets.cadence = {
    rekeyFile = ../../../secrets/store/cadence.age;
    generator.script = { pkgs, ... }: ''
      {
        echo "CADENCE_UUID_SALT=$(${pkgs.openssl}/bin/openssl rand -hex 32)"
        echo "CADENCE_API_RW_KEY=$(${pkgs.openssl}/bin/openssl rand -hex 32)"
      }
    '';
  };

  services.nginx = {
    enable = true;
    virtualHosts = {
      "health.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        locations = {
          "/" = {
            proxyPass = "http://127.0.0.1:8090";
          };
        };
      };
    };
  };

  services.cadence = {
    enable = true;
    listen = "127.0.0.1:8090";
    dataDir = "${dataDirs.level3}/cadence";
    environmentFile = config.age.secrets.cadence.path;
    settings = {
      server = {
        base_url = "https://health.b.nel.family";
        uuid_salt = "\${env:CADENCE_UUID_SALT}";
        api_keys.read_write = [ "\${env:CADENCE_API_RW_KEY}" ];
        oidc = {
          issuer = "https://idm.nel.family/oauth2/openid/cadence";
          client_id = "cadence";
          tier = "read_write";
        };
      };
    };
  };
}
