{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.actual = {
    enable = true;
    package = pkgs.actual-server-fork;
    user = "actual";
    group = "actual";
    settings = {
      hostname = "127.0.0.1";
      port = 5006;
      # Use existing data location from Docker deployment
      dataDir = "${dataDirs.level3}/actual";
      # OpenID configuration (maps to config.json openId object)
      openId = {
        # Kanidm discovery URL
        discoveryURL = "https://idm.nel.family/oauth2/openid/actual/.well-known/openid-configuration";
        client_id = "actual";
        client_secret._secret = config.age.secrets.actual-oauth-client-secret.path;
        server_hostname = "https://budget.h.b.nel.family";
      };
    };
  };

  # Agenix secret for OAuth client secret
  # Use a static group that we add to the service's SupplementaryGroups
  age.secrets.actual-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/actual_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0440";
    group = "actual-secrets";
  };

  # Static user/group for stable ownership of pre-existing data directory
  users.users.actual = {
    isSystemUser = true;
    group = "actual";
  };
  users.groups.actual = {};

  users.groups.actual-secrets = {};

  # Ensure data directory ownership matches the static user
  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/actual 0750 actual actual -"
  ];

  # Add the secrets group to the actual service
  systemd.services.actual.serviceConfig.SupplementaryGroups = [ "actual-secrets" ];
}
