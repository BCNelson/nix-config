{
  config,
  inputs,
  ...
}:
# tendant (Home First) — the household task agent, at tendant.nel.family behind
# authentik SSO.
#
# The service, its Postgres database (peer auth over the unix socket, pgvector),
# its hardening and its nginx vhost all come from the homefirst flake's own
# module (inputs.homefirst.nixosModules.tendant, which also brings in Laya).
# This file is only what is specific to romeo: ports, secrets, which models,
# which identity provider, and where the dumps go.
let
  dataDirs = config.data.dirs;
  host = "tendant.nel.family";

  # 8000/8080 are taken on romeo (paperless, audiobookshelf), which are the
  # upstream defaults for Laya and tendant.
  port = 8095;
  layaPort = 8010;
in {
  imports = [inputs.homefirst.nixosModules.tendant];

  # Prebuilt tendant/web bundles from homefirst's CI. Only a hit when the locked
  # rev has been pushed there; otherwise romeo builds it (pre-seed first).
  nix.settings = {
    extra-substituters = ["https://home-first.cachix.org"];
    extra-trusted-public-keys = ["home-first.cachix.org-1:3dPdFilH9W5daISPBVi5mfHBbnyhevCoBKCFAuvjjUg="];
  };

  ##########################################################################
  # Secrets
  #
  # All of them reach tendant as systemd LoadCredential entries, which systemd
  # reads as root, so the agenix files stay root-owned. tendant resolves its
  # sensitive settings (and ${env:NAME} in the TOML) from $CREDENTIALS_DIRECTORY
  # by name.
  ##########################################################################

  # Device-pairing password: the fallback way in when SSO is unavailable (and
  # how the desktop/mobile apps pair without a browser).
  age.secrets.tendant-password = {
    rekeyFile = ./secrets/tendant_password.age;
    generator.script = "passphrase";
  };

  # AES-256 key sealing source tokens and homefirst.cloud keys INSIDE the
  # database. The pg_dumps below are useless for those rows without it; the
  # master copy lives in the agenix store (and Bitwarden, via the sync).
  age.secrets.tendant-credentials-key = {
    rekeyFile = ./secrets/tendant_credentials_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -base64 32";
  };

  # Shared with authentik on whiskey — same rekeyFile on both sides.
  age.secrets.tendant-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/tendant_auth_client_secret.age;
    generator.script = "alnum";
  };

  services.homefirst.tendant = {
    enable = true;
    httpAddr = "127.0.0.1:${toString port}";

    credentials = {
      TENDANT_PASSWORD = config.age.secrets.tendant-password.path;
      TENDANT_CREDENTIALS_KEY = config.age.secrets.tendant-credentials-key.path;
      TENDANT_OIDC_CLIENT_SECRET = config.age.secrets.tendant-oauth-client-secret.path;
      # Same localhost-only key LibreChat/openclaw present to cli-proxy-api.
      CLI_PROXY_API_KEY = config.age.secrets.cli-proxy-api-key.path;
    };

    settings = {
      server.public_url = "https://${host}";

      # Named endpoints an agent picks from: `llm_connection = "gemma"` in its
      # [[agents]] entry (or the Agents editor), optionally with `model = ...`
      # to override that connection's default model. Agents that name none use
      # agent.connection.
      llm_connections = [
        # ChatGPT subscription models via cli-proxy-api (./cli-proxy-api.nix):
        # gpt-5.5, gpt-5.4-mini, gpt-5.6-{sol,luna,terra}, ... by `model`.
        {
          name = "proxy";
          provider = "openai";
          base_url = "http://127.0.0.1:8317"; # tendant appends /v1/...
          model = "gpt-5.5";
          api_key = "\${env:CLI_PROXY_API_KEY}";
          timeout = "180s";
        }
        # Local Ollama on the B580 (./ollama.nix). gemma4 thinks by default;
        # reasoning off is several times faster. `model = "gemma4:12b-it-qat"`
        # (also pulled) works on this connection too.
        {
          name = "gemma";
          provider = "openai";
          base_url = "http://127.0.0.1:11434";
          model = "gemma4:12b";
          reasoning_effort = "none";
          timeout = "180s";
        }
      ];
      # Default for agents (and the router that picks between them) that name
      # no connection of their own.
      agent.connection = "proxy";
      # The overseer approves or denies gated tool calls (sending email, HTTP
      # requests, ...), so it gets the strongest model, not the local one.
      overseer.connection = "proxy";

      # Already pulled on romeo's Ollama for openclaw (see ./ollama.nix): small
      # enough to stay resident next to gemma4 on the B580.
      embedding = {
        provider = "openai";
        model = "nomic-embed-text";
        base_url = "http://127.0.0.1:11434/v1";
        dimension = 768;
      };

      # tendant has exactly one person, the owner: an admitted SSO identity
      # signs in AS the owner. Hence service_admins, not household.
      auth.oidc = {
        issuer_url = "https://auth.nel.family/application/o/tendant/";
        client_id = "tendant";
        # client_secret comes from the TENDANT_OIDC_CLIENT_SECRET credential.
        allowed_group = "service_admins";
        groups_claim = "groups";
      };
    };

    # Compile gate scripts in the in-app editor (asc, inside this unit's sandbox).
    gatescript.compile = true;

    nginx = {
      enable = true;
      serverName = host;
    };

    # pg_dump nightly onto the vault, where snapshots + borg pick it up. The
    # live database is under /var/lib/postgresql (root disk), like gotosocial's.
    backup = {
      enable = true;
      directory = "${dataDirs.level2}/tendant/db-dumps";
    };
  };

  # Laya: local Jev-format "System One" decision model. The tendant module
  # registers it as the `laya` decision connection and the default for intake
  # triage. CPU-only; first start downloads the weights into /var/lib/laya.
  services.homefirst.laya = {
    enable = true;
    port = layaPort;
    # Leave most of the 32 threads to frigate/ollama/builds.
    threads = 8;
  };

  systemd.services.tendant = {
    after = ["cli-proxy-api.service" "ollama.service"];
    wants = ["cli-proxy-api.service" "ollama.service"];
  };

  # The dump directory is on the vault pool; don't write into an empty
  # mountpoint on the root disk if the timer fires before ZFS has mounted.
  systemd.services.tendant-backup.unitConfig.RequiresMountsFor = ["${dataDirs.level2}/tendant/db-dumps"];
}
