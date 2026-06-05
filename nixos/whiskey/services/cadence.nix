{ config, inputs, lib, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };

  # One entry per scheduled job that should ping cadence.
  # Each gets an agenix-generated UUID pinned in the cadence config and
  # surfaced via `${file:...}` so the same secret can be reused as the
  # pinging service's UUID file.
  checkDefs = [
    # ---- auto-update (nixos-rebuild) per host ----
    { slug = "auto-update-romeo";   period = "1h";  grace = "30m"; tags = [ "auto-update" "host:romeo" ]; }
    { slug = "auto-update-vor";     period = "1h";  grace = "30m"; tags = [ "auto-update" "host:vor" ]; }
    { slug = "auto-update-whiskey"; period = "1h";  grace = "30m"; tags = [ "auto-update" "host:whiskey" ]; }
    { slug = "auto-update-xray";    period = "1h";  grace = "30m"; tags = [ "auto-update" "host:xray" ]; }
    { slug = "auto-update-sierra";  period = "6h";  grace = "2h";  tags = [ "auto-update" "host:sierra" ]; }
    { slug = "auto-update-golf";    period = "6h";  grace = "2h";  tags = [ "auto-update" "host:golf" ]; }
    { slug = "auto-update-bravo";   period = "6h";  grace = "2h";  tags = [ "auto-update" "host:bravo" ]; }
    { slug = "auto-update-qilin";   period = "24h"; grace = "6h";  tags = [ "auto-update" "host:qilin" ]; }
    { slug = "auto-update-berg";    period = "24h"; grace = "6h";  tags = [ "auto-update" "host:berg" ]; }
    { slug = "auto-update-ryuu";    period = "5m";  grace = "5m";  tags = [ "auto-update" "host:ryuu" ]; }

    # ---- borgbackup (every 6h, hardcoded startAt) ----
    { slug = "borgbackup-whiskey-level1"; period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:whiskey" ]; }
    { slug = "borgbackup-whiskey-level2"; period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:whiskey" ]; }
    { slug = "borgbackup-whiskey-level3"; period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:whiskey" ]; }
    { slug = "borgbackup-romeo-level1";   period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:romeo" ]; }
    { slug = "borgbackup-romeo-level2";   period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:romeo" ]; }
    { slug = "borgbackup-romeo-level3";   period = "6h"; grace = "2h"; tags = [ "backup" "borg" "host:romeo" ]; }

    # ---- syncoid (hourly default) ----
    { slug = "syncoid-romeo-NelsonData"; period = "1h"; grace = "30m"; tags = [ "backup" "syncoid" "host:romeo" ]; }
    { slug = "syncoid-romeo-level1";     period = "1h"; grace = "30m"; tags = [ "backup" "syncoid" "host:romeo" ]; }
    { slug = "syncoid-romeo-level2";     period = "1h"; grace = "30m"; tags = [ "backup" "syncoid" "host:romeo" ]; }
    { slug = "syncoid-vor-NelsonData";   period = "1h"; grace = "30m"; tags = [ "backup" "syncoid" "host:vor" ]; }

    # ---- zfs scrub (monthly default) ----
    { slug = "zfs-scrub-romeo"; period = "720h"; grace = "168h"; tags = [ "zfs" "scrub" "host:romeo" ]; }
    { slug = "zfs-scrub-vor";   period = "720h"; grace = "168h"; tags = [ "zfs" "scrub" "host:vor" ]; }

    # ---- container / service updates ----
    { slug = "auto-update-services-romeo"; period = "1h";  grace = "30m"; tags = [ "auto-update" "containers" "host:romeo" ]; }
    { slug = "podman-auto-update-romeo";   period = "24h"; grace = "6h";  tags = [ "auto-update" "containers" "host:romeo" ]; }
  ];

  # agenix secrets are keyed by attribute name, so swap '-' for '_'.
  secretAttr = c: "cadence_check_" + lib.replaceStrings [ "-" ] [ "_" ] c.slug;
  checksDir = ../../../secrets/store/cadence/checks;

  uuidSecrets = lib.listToAttrs (map (c: {
    name = secretAttr c;
    value = {
      rekeyFile = checksDir + "/${c.slug}.age";
      generator.script = { pkgs, ... }: "${pkgs.util-linux}/bin/uuidgen";
      owner = config.services.cadence.user;
      group = config.services.cadence.group;
      mode = "0440";
    };
  }) checkDefs);

  mkCheck = c: {
    inherit (c) slug;
    period = c.period or null;
    cron = c.cron or null;
    grace = c.grace or null;
    tags = c.tags or [ ];
    uuid = "\${file:${config.age.secrets.${secretAttr c}.path}}";
  };
in
{
  imports = [ inputs.cadence.nixosModules.cadence ];

  age.secrets = uuidSecrets // {
    cadence = {
      rekeyFile = ../../../secrets/store/cadence.age;
      generator.script = { pkgs, ... }: ''
        {
          echo "CADENCE_UUID_SALT=$(${pkgs.openssl}/bin/openssl rand -hex 32)"
          echo "CADENCE_API_RW_KEY=$(${pkgs.openssl}/bin/openssl rand -hex 32)"
        }
      '';
    };
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
      checks = map mkCheck checkDefs;
    };
  };
}
