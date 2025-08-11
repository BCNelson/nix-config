{ config, pkgs, ... }:
{
  services.nginx = {
    enable = true;
    virtualHosts = {
      "idm.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 512M;
        '';
        locations = {
          "/" = {
            proxyPass = "https://localhost:3001";
          };
        };
      };
    };
  };

  users.groups.kanidm = {
    members = [ "nginx" ];
  };

  security.acme = {
    certs."idm.nel.family" = {
      group = "kanidm";
    };
  };

  services.kanidm = {
    enableServer = true;
    package = pkgs.kanidmWithSecretProvisioning;
    serverSettings = {
      origin = "https://idm.nel.family";
      domain = "nel.family";
      tls_key = config.security.acme.certs."idm.nel.family".directory + "/key.pem";
      tls_chain = config.security.acme.certs."idm.nel.family".directory + "/fullchain.pem";
      role = "WriteReplica";
      bindaddress = "127.0.0.1:3001";
    };
    provision = {
      enable = true;
      groups = {
        "household" = {
          members = [
            "bcnelson"
            "haleylyn"
          ];
          overwriteMembers = true;
        };
        "service_admins" = {
          members = [
            "bcnelson"
          ];
          overwriteMembers = true;
        };
      };
      systems.oauth2 = {
        "audiobookshelf" = {
          displayName = "Audiobookshelf";
          originUrl = "https://audiobooks.nel.family/";
          originLanding = "https://audiobooks.nel.family/";
          scopeMaps = {
            "household" = [ "email" "openid" "profile"];
          };
          preferShortUsername = true;
        };
        "immich" = {
          displayName = "Immich";
          originUrl = [
            "app.immich://"
            "https://photos.h.b.nel.family/user-settings"
            "https://photos.h.b.nel.family/auth/login"
          ];
          originLanding = "https://photos.h.b.nel.family/";
          scopeMaps = {
            "household" = [ "email" "openid" "profile"];
          };
          allowInsecureClientDisablePkce = true;
          preferShortUsername = true;
        };
        "vikunja" = {
          displayName = "Vikunja";
          originUrl = "https://todo.nel.family/auth/openid/";
          originLanding = "https://todo.nel.family/";
          scopeMaps = {
            "household" = [ "email" "openid" "profile"];
          };
          allowInsecureClientDisablePkce = true;
        };
        "mealie" = {
          displayName = "Mealie";
          originUrl = "https://recipes.nel.family/login";
          originLanding = "https://recipes.nel.family/";
          scopeMaps = {
            "household" = [ "email" "groups" "openid" "profile"];
          };
        };
        "jellyfin" = {
          displayName = "Jellyfin";
          originUrl = "https://jellyfin.example.com/sso/OID/redirect/kanidm";
          originLanding = "https://media.nel.family/";
          scopeMaps = {
            "household" = [ "email" "groups" "openid" "profile"];
          };
        };
        "paperless" = {
          displayName = "Paperless";
          originUrl = "https://docs.h.b.nel.family/accounts/oidc/kanidm/login/callback";
          originLanding = "https://docs.h.b.nel.family/";
          scopeMaps = {
            "service_admins" = [ "email" "groups" "openid" "profile"];
          };
        };
        "grafana" = {
          displayName = "Grafana";
          originUrl = "https://grafana.b.nel.family/login/generic_oauth";
          originLanding = "https://grafana.b.nel.family/";
          scopeMaps = {
            "service_admins" = [ "email" "groups" "openid" "profile"];
          };
          claimMaps = {
            "grafana_role" = {
              valuesByGroup = {
                "service_admins" = [ "GrafanaAdmin" ];
              };
            };
          };
        };
      };
      persons = {
        "bcnelson" = {
          present = true;
          displayName = "Bradley Nelson";
          mailAddresses = [ "bradley@nel.family" ];
          groups = [ "household" "service_admins" ];
        };
        "haleylyn" = {
          present = true;
          displayName = "Haley Nelson";
          mailAddresses = [ "haley.lyn15@gmail.com" ];
          groups = [ "household" ];
        };
      };
    };
    enableClient = true;
    clientSettings = {
      uri = "https://127.0.0.1:3001";
    };
  };
}
