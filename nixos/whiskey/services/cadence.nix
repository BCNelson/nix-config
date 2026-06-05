{ config, inputs, pkgs, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };
  # The cadence NixOS module's typed `settings` schema doesn't yet expose
  # `server.oidc` (added in the daemon at 14cc4b6). Use the documented
  # `extraConfigFiles` escape hatch — the daemon merges YAML over `settings`.
  oidcConfig = pkgs.writeText "cadence-oidc.yaml" ''
    server:
      oidc:
        issuer: https://idm.nel.family/oauth2/openid/cadence
        client_id: cadence
        tier: read_write
  '';
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
      };
    };
    extraConfigFiles = [ oidcConfig ];
  };
}
