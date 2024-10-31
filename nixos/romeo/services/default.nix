{ libx, dataDirs, pkgs, ... }:
let
  porkbun_api_creds = libx.getSecretWithDefault ../sensitive.nix "porkbun_api" {
    api_key = "";
    secret_key = "";
  };
  jellyfin = import ./defs/jellyfin.nix { inherit dataDirs; };
  audiobooks = import ./defs/audiobooks.nix { inherit dataDirs; };
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs libx pkgs; };
  mealie = import ./defs/mealie.nix { inherit dataDirs libx; };
  syncthing = import ./defs/syncthing.nix { inherit dataDirs; };
  foundryvtt = import ./defs/foundryvtt.nix { inherit dataDirs libx; };
  fastenhealth = import ./defs/fastenhealth.nix { inherit dataDirs; };
  homebox = import ./defs/homebox.nix { inherit dataDirs; };
  immich = import ./defs/immich.nix { inherit dataDirs libx; };
  paperless = import ./defs/paperless.nix { inherit dataDirs libx; };
  tubearchivist = import ./defs/tubearchivist.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dockerComposeDefinition = {
      services = builtins.foldl' (a: b: a // b) { } [
        jellyfin
        audiobooks
        nextcloud
        vikunja
        mealie
        syncthing
        foundryvtt
        fastenhealth
        homebox
        immich
        paperless
        tubearchivist
      ];
    };
  };

  security.acme = {
    acceptTerms = true;
    defaults = {
      email = "admin@nel.family";
      dnsProvider = "porkbun";
      environmentFile = "${pkgs.writeText "porkbun-creds" ''
        PORKBUN_API_KEY=${porkbun_api_creds.api_key}
        PORKBUN_SECRET_API_KEY=${porkbun_api_creds.secret_key}
      ''}";
    };
  };

  services.nginx = {
    enable = true;
    recommendedGzipSettings = true;
    recommendedZstdSettings = true;
    recommendedTlsSettings = true;
    recommendedProxySettings = true;
    recommendedOptimisation = true;
    virtualHosts = {
      "media.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8096";
            extraConfig = ''
              proxy_set_header Range $http_range;
              proxy_set_header If-Range $http_if_range;
            '';
          };
          "~ (/jellyfin)?/socket" = {
            proxyWebsockets = true;
            proxyPass = "http://localhost:8096";
          };
        };
      };
      "audiobooks.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8080";
          };
        };
      };
      "nextcloud.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "https://localhost:8443";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "todo.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:3456";
          };
        };
      };
      "recipes.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:9000";
          };
        };
      };
      "syncthing.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8384";
          };
        };
      };
      "pathfinder.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:30000";
          };
        };
      };
      "health.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8081";
          };
        };
      };
      "inventory.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:7745";
          };
        };
      };
      "nel.to" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/a" = {
            return = "301 https://inventory.h.b.nel.family$request_uri";
          };
        };
      };
      "photos.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:2283";
          };
          "~ (/immich)?/api" = {
            proxyPass = "http://localhost:2283";
          };
        };
      };
      "docs.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8000";
          };
        };
      };
      "tube.romeo.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8001";
          };
        };
      };
    };
  };

  networking.firewall.allowedTCPPorts = [ 80 443 ];
}
