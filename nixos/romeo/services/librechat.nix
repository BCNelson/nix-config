{ config, ... }:
let
  dataDirs = config.data.dirs;
  port = 3080;
  domain = "ai.h.b.nel.family";
in
{
  # LibreChat replaces open-webui on ai.h.b.nel.family. The old open-webui data
  # is untouched at ${dataDirs.level5}/open-webui if it ever needs recovering.

  # Symmetric key + IV LibreChat uses to encrypt user-supplied credentials (API
  # keys stored per-user) at rest in Mongo. CREDS_KEY is 32 bytes, CREDS_IV is
  # 16 bytes, both hex-encoded -- LibreChat validates the lengths at startup.
  # Rotating either one makes every stored user credential undecryptable.
  age.secrets.librechat-creds-key = {
    rekeyFile = ./secrets/librechat_creds_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32 | ${pkgs.coreutils}/bin/tr -d '\\n'";
  };
  age.secrets.librechat-creds-iv = {
    rekeyFile = ./secrets/librechat_creds_iv.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 16 | ${pkgs.coreutils}/bin/tr -d '\\n'";
  };

  # Session signing. Rotating these just logs everyone out.
  age.secrets.librechat-jwt-secret = {
    rekeyFile = ./secrets/librechat_jwt_secret.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32 | ${pkgs.coreutils}/bin/tr -d '\\n'";
  };
  age.secrets.librechat-jwt-refresh-secret = {
    rekeyFile = ./secrets/librechat_jwt_refresh_secret.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32 | ${pkgs.coreutils}/bin/tr -d '\\n'";
  };

  age.secrets.meilisearch-master-key = {
    rekeyFile = ./secrets/meilisearch_master_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -base64 36 | ${pkgs.coreutils}/bin/tr -d '\\n'";
  };

  services.librechat = {
    enable = true;
    dataDir = "${dataDirs.level5}/librechat";

    # Pulls in services.mongodb (unfree SSPL; channelsConfig.allowUnfree covers
    # it) and points MONGO_URI at mongodb://localhost:27017.
    enableLocalDB = true;

    # Wires SEARCH=true, MEILI_HOST, and the MEILI_MASTER_KEY credential for us.
    meilisearch.enable = true;

    # Do NOT set openFirewall: the module's implementation references a
    # nonexistent `cfg.port` and fails to evaluate. nginx fronts this anyway.
    env = {
      HOST = "127.0.0.1";
      PORT = port;
      DOMAIN_CLIENT = "https://${domain}";
      DOMAIN_SERVER = "https://${domain}";

      # Needed to create the first account, since users live in Mongo and there
      # is no declarative way to seed one. Flip to false once you have signed up.
      ALLOW_REGISTRATION = true;
      ALLOW_EMAIL_LOGIN = true;
      ALLOW_SOCIAL_LOGIN = false;

      # RAG_API_URL is deliberately unset: LibreChat's rag_api (Python +
      # pgvector) has no NixOS module, so there is no document-embedding
      # pipeline here. Uploads still work for vision-capable models.
    };

    credentials = {
      CREDS_KEY = config.age.secrets.librechat-creds-key.path;
      CREDS_IV = config.age.secrets.librechat-creds-iv.path;
      JWT_SECRET = config.age.secrets.librechat-jwt-secret.path;
      JWT_REFRESH_SECRET = config.age.secrets.librechat-jwt-refresh-secret.path;
      # Authenticates LibreChat to the local cli-proxy-api, which holds the
      # actual ChatGPT/Codex OAuth credential. See ./cli-proxy-api.nix.
      # The module exports every credential into the process environment before
      # exec, which is what makes ''${PROXY_API_KEY} below resolve.
      PROXY_API_KEY = config.age.secrets.cli-proxy-api-key.path;
    };

    # Becomes librechat.yaml.
    settings = {
      # The module defaults to 1.2.1, which LibreChat 0.8.6 logs as outdated.
      version = "1.3.12";
      cache = true;
      interface = {
        modelSelect = true;
        parameters = true;
        presets = true;
      };
      endpoints.custom = [
        {
          name = "ChatGPT";
          apiKey = "\${PROXY_API_KEY}";
          baseURL = "http://127.0.0.1:8317/v1";
          models = {
            # fetch = true populates the picker from cli-proxy-api's /v1/models,
            # so this list only supplies the fallback/preselected model. It has
            # to name something the proxy actually serves -- the catalog is
            # gpt-5.4{,-mini}, gpt-5.5, gpt-5.6-{luna,sol,terra}, plus the codex
            # and image models. There is no plain "gpt-5".
            default = [ "gpt-5.5" ];
            fetch = true;
          };
          titleConvo = true;
          titleModel = "current_model";
          modelDisplayLabel = "ChatGPT";
        }
        {
          name = "Ollama";
          # Ollama ignores the value but LibreChat requires a non-empty key.
          apiKey = "ollama";
          baseURL = "http://127.0.0.1:11434/v1";
          models = {
            default = [ "qwen3.5:9b" ];
            fetch = true;
          };
          titleConvo = true;
          titleModel = "current_model";
          modelDisplayLabel = "Ollama";
        }
      ];
    };
  };

  systemd.services.librechat = {
    after = [ "zfs-import.target" "mongodb.service" "cli-proxy-api.service" "ollama.service" ];
    requires = [ "zfs-import.target" "mongodb.service" ];
    wants = [ "cli-proxy-api.service" "ollama.service" ];
  };

  # Keep the database on the vault next to the app data rather than the module
  # default of /var/db/mongodb on the root pool.
  services.mongodb.dbpath = "${dataDirs.level5}/librechat-mongodb";
  systemd.services.mongodb = {
    after = [ "zfs-import.target" ];
    requires = [ "zfs-import.target" ];
  };

  # The Meilisearch index is derived from Mongo and can be rebuilt, so it stays
  # on the module default (/var/lib/meilisearch) rather than the vault.
  services.meilisearch.masterKeyFile = config.age.secrets.meilisearch-master-key.path;

  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations = {
      "/" = {
        proxyPass = "http://127.0.0.1:${toString port}";
        proxyWebsockets = true;
      };
    };
  };
}
