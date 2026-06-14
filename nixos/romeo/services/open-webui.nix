{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  # Secret for JWT token generation
  age.secrets.open-webui-secret-key = {
    rekeyFile = ./secrets/open_webui_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  # OIDC client secret, shared with the Authentik provider on whiskey (same
  # rekeyFile). Injected via EnvironmentFile as OAUTH_CLIENT_SECRET rather than
  # the world-readable systemd Environment= block.
  age.secrets.open-webui-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/open_webui_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.open-webui-env = {
    vars = {
      SECRET_KEY = config.age.secrets.open-webui-secret-key.path;
      OAUTH_SECRET = config.age.secrets.open-webui-oauth-client-secret.path;
    };
    content = ''
      WEBUI_SECRET_KEY=$SECRET_KEY
      OAUTH_CLIENT_SECRET=$OAUTH_SECRET
    '';
  };

  services.open-webui = {
    enable = true;
    host = "127.0.0.1";
    port = 8085;
    stateDir = "${dataDirs.level5}/open-webui";
    environment = {
      OLLAMA_BASE_URL = "http://127.0.0.1:11434";
      DATA_DIR = "${dataDirs.level5}/open-webui";

      # Public URL, used to build the OIDC redirect URI
      # (https://ai.h.b.nel.family/oauth/oidc/callback).
      WEBUI_URL = "https://ai.h.b.nel.family";

      # --- Authentik OIDC (Family AI) -------------------------------------
      # Per-application issuer on auth.nel.family; discovery doc lives at
      # <issuer>/.well-known/openid-configuration. Mirrors gotosocial.nix.
      OPENID_PROVIDER_URL = "https://auth.nel.family/application/o/open-webui/.well-known/openid-configuration";
      OAUTH_CLIENT_ID = "open-webui";
      OAUTH_PROVIDER_NAME = "Authentik";
      # authentik has no dedicated "groups" scope; group membership rides inside
      # the default "profile" scope claim, so requesting it is enough.
      OAUTH_SCOPES = "openid email profile";
      # Auto-provision family accounts on first OIDC login. Access is already
      # gated at authentik (the application's policy bindings only admit
      # household + extended_family), so anyone who can sign in belongs here.
      ENABLE_OAUTH_SIGNUP = "true";
      # Link OIDC logins to the pre-existing local admin account by email.
      OAUTH_MERGE_ACCOUNTS_BY_EMAIL = "true";
      # New OIDC users land as full users (not "pending" approval) since
      # authentik has already vetted group membership.
      DEFAULT_USER_ROLE = "user";
      # Close local self-registration now that the vhost is publicly reachable;
      # the login form stays available as an admin fallback if OIDC breaks.
      ENABLE_SIGNUP = "false";
    };
  };

  systemd.services.open-webui = {
    after = [ "zfs-import.target" ];
    requires = [ "zfs-import.target" ];
    serviceConfig = {
      EnvironmentFile = config.age-template.files.open-webui-env.path;
      ReadWritePaths = [ "${dataDirs.level5}/open-webui" ];
    };
  };

  # Publicly reachable so off-network family can complete the Authentik OIDC
  # login. Access control now lives in the app + IdP: authentik only issues
  # tokens to household + extended_family (application policy bindings), local
  # self-registration is disabled (ENABLE_SIGNUP=false), and WEBUI_AUTH is on
  # by default — replacing the previous Tailscale/LAN allow-list.
  services.nginx.virtualHosts."ai.h.b.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
    '';
    locations = {
      "/" = {
        proxyPass = "http://127.0.0.1:8085";
        proxyWebsockets = true;
      };
    };
  };
}
