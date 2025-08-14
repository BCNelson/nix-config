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

  age.secrets.kanidm-admin-password = {
    rekeyFile = ./secrets/kanidm_admin_password.age;
    generator.script = "passphrase";
    mode= "0400";
    owner = "kanidm";
  };

  age.secrets.kanidm-idm-admin-password = {
    rekeyFile = ./secrets/kanidm_idm_admin_password.age;
    generator.script = "passphrase";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.audiobookshelf-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/audiobookshelf_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.immich-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/immich_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.vikunja-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/vikunja_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.mealie-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/mealie_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.jellyfin-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/jellyfin_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.paperless-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/paperless_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
  };

  age.secrets.grafana-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/grafana_auth_client_secret.age;
    generator.script = "alnum";
    mode = "0400";
    owner = "kanidm";
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
      adminPasswordFile = config.age.secrets.kanidm-admin-password.path;
      idmAdminPasswordFile = config.age.secrets.kanidm-idm-admin-password.path;
      groups = {
        "household" = {
          members = [
            "bcnelson"
            "haleylyn"
          ];
          overwriteMembers = true;
        };
        "extended_family" = {
          members = [
            "cwnelson"
          ];
          overwriteMembers = false;
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
          originUrl = [ 
            "https://audiobooks.nel.family/auth/openid/callback"
            "https://audiobooks.nel.family/auth/openid/mobile-redirect"
          ];
          originLanding = "https://audiobooks.nel.family/";
          scopeMaps = {
            "household" = [ "email" "openid" "profile"];
            "extended_family" = [ "email" "openid" "profile"];
          };
          basicSecretFile = config.age.secrets.audiobookshelf-oauth-client-secret.path;
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
          basicSecretFile = config.age.secrets.immich-oauth-client-secret.path;
          preferShortUsername = true;
        };
        "vikunja" = {
          displayName = "Vikunja";
          originUrl = "https://todo.nel.family/auth/openid/";
          originLanding = "https://todo.nel.family/";
          scopeMaps = {
            "household" = [ "email" "openid" "profile"];
          };
          basicSecretFile = config.age.secrets.vikunja-oauth-client-secret.path;
          allowInsecureClientDisablePkce = true;
        };
        "mealie" = {
          displayName = "Mealie";
          originUrl = "https://recipes.nel.family/login";
          originLanding = "https://recipes.nel.family/";
          scopeMaps = {
            "household" = [ "email" "groups" "openid" "profile"];
            "extended_family" = [ "email" "groups" "openid" "profile"];
          };
          basicSecretFile = config.age.secrets.mealie-oauth-client-secret.path;
        };
        "jellyfin" = {
          displayName = "Jellyfin";
          originUrl = "https://jellyfin.example.com/sso/OID/redirect/kanidm";
          originLanding = "https://media.nel.family/";
          scopeMaps = {
            "household" = [ "email" "groups" "openid" "profile"];
            "extended_family" = [ "email" "groups" "openid" "profile"];
          };
          basicSecretFile = config.age.secrets.jellyfin-oauth-client-secret.path;
        };
        "paperless" = {
          displayName = "Paperless";
          originUrl = "https://docs.h.b.nel.family/accounts/oidc/kanidm/login/callback/";
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
          basicSecretFile = config.age.secrets.grafana-oauth-client-secret.path;
        };
      };
      persons = {
        "bcnelson" = {
          present = true;
          displayName = "Bradley Nelson";
          mailAddresses = [ "bradley@nel.family" ];
          groups = [
            "household"
            "service_admins"
          ];
        };
        "haleylyn" = {
          present = true;
          displayName = "Haley Nelson";
          mailAddresses = [ "haley.lyn15@gmail.com" ];
          groups = [ "household" ];
        };
        "cwnelson" = {
          present = true;
          displayName = "Carter Nelson";
          mailAddresses = [ "cartern215@gmail.com" ];
          groups = [ "extended_family" ];
        };
      };
    };
    enableClient = true;
    clientSettings = {
      uri = "https://127.0.0.1:3001";
    };
  };
}
