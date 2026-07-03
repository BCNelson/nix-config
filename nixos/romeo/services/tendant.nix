{ config, inputs, lib, ... }:
{
  imports = [ inputs.homefirst-modules.nixosModules.tendant ];

  # base64 AES-256 key for the credentials vault (TENDANT_CREDENTIALS_KEY).
  # Guarded on enable: owner/group default to the "tendant" system user, which
  # the upstream module only creates when enabled. Activating the secret while
  # disabled chowns to a nonexistent user ("chown: invalid user: tendant:tendant").
  age.secrets.tendant-credentials-key = lib.mkIf config.services.homefirst.tendant.enable {
    rekeyFile = ./secrets/tendant_credentials_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -base64 32";
    owner = config.services.homefirst.tendant.user;
    group = config.services.homefirst.tendant.group;
    mode = "0440";
  };

  # Static, reusable device-pairing password (TENDANT_PASSWORD). Presented to
  # the pairDevice mutation; each device is then issued its own session token.
  # Retrieve the value to pair a device with: `agenix decrypt secrets/store/... `
  # (or read /run/agenix/tendant-password on the host).
  age.secrets.tendant-password = lib.mkIf config.services.homefirst.tendant.enable {
    rekeyFile = ./secrets/tendant_password.age;
    # Human-typeable passphrase (xkcdpass, 6 words) — entered during pairing.
    generator.script = "passphrase";
    owner = config.services.homefirst.tendant.user;
    group = config.services.homefirst.tendant.group;
    mode = "0440";
    bitwarden = {
      name = "Tendant Device Pairing";
      username = "owner";
      uris = {
        uri = "https://tendant-bcn.h.b.nel.family";
        matchType = "host";
      };
      notes = "Static device-pairing password for tendant on romeo-2. Enter it as the password in the pairDevice flow (displayName is any label for the device); each device then gets its own revocable session token.";
    };
  };

  services.homefirst.tendant = {
    enable = true;
    httpAddr = "127.0.0.1:8095";
    # Provisions a local PostgreSQL and a tendant DB/role; connects over the
    # local unix socket with peer auth.
    database.createLocally = true;
    credentials = {
      TENDANT_CREDENTIALS_KEY = config.age.secrets.tendant-credentials-key.path;
      TENDANT_PASSWORD = config.age.secrets.tendant-password.path;
    };
    # Non-secret env vars. Point the overseer at the host's local ollama via its
    # OpenAI-compatible endpoint. Env keys map TOML dots to double underscores:
    # TENDANT_<SECTION>__<KEY> == overseer.openai.base_url, etc.
    settings = {
      TENDANT_OVERSEER__PROVIDER = "openai";
      TENDANT_OVERSEER__MODEL_ID = "qwen3:4b";
      TENDANT_OVERSEER__OPENAI__BASE_URL = "http://127.0.0.1:11434/v1";
      # ollama ignores the key, but the openai client requires a non-empty value.
      TENDANT_OVERSEER__OPENAI__API_KEY = "ollama";
    };
  };

  # The overseer talks to the local ollama daemon; start tendant after it.
  # Guarded on enable: when disabled the upstream module provides no ExecStart,
  # and an unconditional override here would leave a unit with only wants/after
  # ("Service has no ExecStart=. Refusing." -> bad unit file setting).
  systemd.services.tendant = lib.mkIf config.services.homefirst.tendant.enable {
    wants = [ "ollama.service" ];
    after = [ "ollama.service" ];
  };

  services.nginx = lib.mkIf config.services.homefirst.tendant.enable {
    enable = true;
    virtualHosts."tendant-bcn.h.b.nel.family" = {
      forceSSL = true;
      enableACME = true;
      acmeRoot = null;
      http2 = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8095";
        proxyWebsockets = true;
      };
    };
  };
}
