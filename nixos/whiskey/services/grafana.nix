{ config, ... }:
let
  cfg = config.services.grafana;
  authentik = "https://auth.nel.family/application/o";
in
{
  age.secrets.grafana-secret-key = {
    rekeyFile = ./secrets/grafana_secret_key.age;
    generator.script = "alnum";
    owner = "grafana";
  };

  services.grafana = {
    enable = true;

    settings = {
      security.secret_key = "$__file{${config.age.secrets.grafana-secret-key.path}}";

      server = {
        root_url = "https://grafana.b.nel.family";
        enable_gzip = true;
        enforce_domain = true;
        domain = "grafana.b.nel.family";
        http_port = 2342;
        http_addr = "127.0.0.1";
      };

      # Local sign-up is off; accounts come from authentik, which only lets
      # service_admins through to this application (blueprints/grafana.yaml).
      users.allow_sign_up = false;

      "auth.generic_oauth" = {
        enabled = true;
        name = "authentik";
        client_id = "grafana";
        # Declared in authentik.nix (group-readable by grafana).
        client_secret = "$__file{${config.age.secrets.grafana-oauth-client-secret.path}}";
        scopes = "openid profile email";
        auth_url = "${authentik}/authorize/";
        token_url = "${authentik}/token/";
        api_url = "${authentik}/userinfo/";
        use_pkce = true;
        use_refresh_token = true;
        allow_sign_up = true;

        # Email doubles as the login so the first sign-in adopts the existing
        # bradley@nel.family user; with preferred_username (bcnelson) Grafana
        # would try to create a second user and fail on the duplicate email.
        login_attribute_path = "email";
        email_attribute_path = "email";
        name_attribute_path = "name";
        groups_attribute_path = "groups";

        role_attribute_path = "contains(groups[*], 'service_admins') && 'GrafanaAdmin' || 'Viewer'";
        allow_assign_grafana_admin = true;
      };
    };

    provision = {
      enable = true;

      datasources.settings = {
        # Drop datasources that were provisioned before but are no longer listed.
        prune = true;

        # The UI-created datasources from before this was declarative; clear
        # them so the entries below own the names. A no-op once they are gone.
        deleteDatasources = [
          { name = "nel.family Promethus"; orgId = 1; }
          { name = "loki"; orgId = 1; }
        ];

        datasources = [
          {
            name = "Prometheus";
            uid = "prometheus";
            type = "prometheus";
            url = "http://127.0.0.1:${toString config.services.prometheus.port}";
            isDefault = true;
            jsonData.httpMethod = "POST";
          }
          {
            name = "Loki";
            uid = "loki";
            type = "loki";
            url = "http://127.0.0.1:${toString config.services.loki.configuration.server.http_listen_port}";
          }
        ];
      };
    };
  };

  # nginx reverse proxy
  services.nginx.virtualHosts.${cfg.settings.server.domain} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString cfg.settings.server.http_port}";
      proxyWebsockets = true;
    };
  };
}
