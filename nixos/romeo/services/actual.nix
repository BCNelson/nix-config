{ config, ... }:
{
  services.actual = {
    enable = true;
    settings = {
      hostname = "127.0.0.1";
      port = 5006;
      # Use existing data location from Docker deployment
      dataDir = "/data/level3/actual";
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
  # Note: Don't specify owner since actual uses DynamicUser=true
  # The preStart script runs as root before dropping privileges
  age.secrets.actual-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/actual_auth_client_secret.age;
    generator.script = "alnum";
  };
}
