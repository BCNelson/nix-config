{
  config,
  pkgs,
  lib,
  ...
}: let
  port = 18789; # openclaw gateway default
  domain = "openclaw.h.b.nel.family";
  stateDir = "/var/lib/openclaw";
  workspace = "${stateDir}/workspace";

  # pkgs/openclaw.nix, not nixpkgs' openclaw: nixpkgs tracks the 2026.6
  # maintenance line, which never received the `/pair qr` "Media failed" fix
  # (openclaw/openclaw#97933). The `additions` overlay shadows the nixpkgs
  # attribute, so `pkgs.openclaw` here is ours. Version, hashes and the reason
  # for dropping the insecure marker are all documented there.
  inherit (pkgs) openclaw;

  # OPENCLAW_NIX_MODE is what makes this file, rather than the running daemon,
  # the source of truth. Without it openclaw treats openclaw.json as mutable and
  # rewrites it atomically (rename onto the path) during onboarding, `config
  # set`, `doctor --fix` and plugin install -- which would either clobber a
  # store path or, for a symlink, replace the *target*. Nix mode makes the
  # config immutable and disables every self-mutation flow, so pointing
  # OPENCLAW_CONFIG_PATH straight at a read-only store file is safe.
  #
  # Consequence worth knowing: `openclaw config set ...` and `openclaw plugins
  # install ...` will refuse to run. Editing this file and rebuilding is the
  # only way to change configuration, which is the point.
  #
  # Startup logs one harmless warning as a result:
  #   failed to promote config last-known-good backup: EROFS: read-only file
  #   system, open '/nix/store/...-openclaw.json.last-good'
  # openclaw keeps a .last-good sidecar next to the config to roll back a bad
  # self-edit. There are no self-edits to roll back here and the store is
  # read-only, so the warning is expected and the gateway continues to "ready".

  # JSON is a strict subset of the JSON5 the gateway parses, so the normal
  # generator is fine -- no custom writer needed.
  openclawConfig = (pkgs.formats.json {}).generate "openclaw.json" {
    gateway = {
      inherit port;
      # Mandatory, and easy to miss: the gateway refuses to start with
      # "Gateway start blocked: set gateway.mode=local" when this key is absent.
      # Normally `openclaw onboard` stamps it, but Nix mode disables onboarding
      # and every other config writer, so it has to be declared here.
      mode = "local";
      # nginx terminates TLS and owns the allowlist; the gateway itself only
      # ever needs to answer on loopback. "tailnet" would bind the tailscale
      # address directly and bypass the vhost's allow/deny rules.
      bind = "loopback";
      # Interpolated from the process environment at config load time, so the
      # token never lands in the world-readable store. Only uppercase
      # [A-Z_][A-Z0-9_]* names are substituted, and a missing/empty var is a
      # hard error at startup rather than a silently unauthenticated gateway.
      auth.token = "\${OPENCLAW_GATEWAY_TOKEN}";

      # Without this the Control UI loads but the gateway rejects its WebSocket
      # with "Browser origin not allowed".
      #
      # The gateway auto-trusts same-origin UI loads from loopback, RFC1918,
      # link-local, *.local, *.ts.net and Tailscale CGNAT hosts -- which is why
      # this is not needed for a plain http://<lan-ip>:18789 load. It IS needed
      # here because the vhost is reached by a public DNS name
      # (openclaw.h.b.nel.family), and openclaw classifies the origin by the
      # name in the browser's address bar, not by the address it resolves to.
      # nginx being LAN/tailnet-only makes no difference to that check.
      #
      # Exact origin match: scheme + host, no trailing slash, no port (443 is
      # implicit for https). The alternative upstream offers is
      # dangerouslyAllowHostHeaderOriginFallback, which trusts the Host header
      # an attacker controls -- an explicit allowlist is the safe form.
      controlUi.allowedOrigins = ["https://${domain}"];
    };

    models.providers.cliproxy = {
      # Same cli-proxy-api that librechat.nix and goose.nix already talk to: it
      # holds the ChatGPT OAuth credential and presents an OpenAI-compatible
      # surface. Note this is a *custom* provider id rather than the built-in
      # `openai` one -- upstream routes `openai/*` agent turns through the
      # native Codex app-server runtime, which wants real Codex auth and would
      # ignore baseUrl entirely. A custom `openai-completions` provider is the
      # documented way to point at a local OpenAI-compatible endpoint.
      baseUrl = "http://127.0.0.1:8317/v1";
      apiKey = "\${CLI_PROXY_API_KEY}";
      api = "openai-completions";
      # Requests to private addresses are guarded by default. A custom provider
      # implicitly trusts its own baseUrl origin, but state this explicitly so a
      # future port change does not silently start failing the guard.
      request.allowPrivateNetwork = true;
      # Catalog must match what cli-proxy-api actually serves. Verified against
      # its /v1/models on romeo: gpt-5.4, gpt-5.4-mini, gpt-5.5, gpt-5.6-{sol,
      # luna,terra}, plus image/codex entries. There is no plain "gpt-5".
      #
      # Every field below is REQUIRED by openclaw's schema for a custom
      # provider. Omitting them fails startup with
      # "models.providers.cliproxy.models.N.name: Invalid input" -- the schema
      # reports only the first missing key per entry, so a partial fix just
      # surfaces the next one. Built-in providers fetch this metadata from a
      # remote catalog; a custom provider has to declare it inline.
      #
      # cost is 0 across the board because this rides the ChatGPT subscription
      # through cli-proxy-api rather than per-token API billing -- these numbers
      # only feed openclaw's spend reporting, and a nonzero rate here would
      # invent charges that do not exist. contextWindow/maxTokens are the
      # published GPT-5-family figures; they drive compaction and session
      # budgeting only, so adjust them if a model's real limits differ.
      models =
        map (m: {
          inherit (m) id name;
          reasoning = true;
          input = ["text" "image"];
          cost = {
            input = 0;
            output = 0;
            cacheRead = 0;
            cacheWrite = 0;
          };
          contextWindow = 400000;
          maxTokens = 128000;
        }) [
          {
            id = "gpt-5.5";
            name = "GPT-5.5";
          }
          {
            id = "gpt-5.4";
            name = "GPT-5.4";
          }
          {
            id = "gpt-5.4-mini";
            name = "GPT-5.4 mini";
          }
        ];
    };

    # ../ollama.nix on this same host, reached over loopback exactly like every
    # other consumer (librechat.nix, tendant.nix, goose.nix). The model list is
    # deliberately absent: openclaw discovers it live from the daemon, so
    # `services.ollama.loadModels` stays the single source of truth for which
    # models exist and this file never has to be kept in sync with it.
    #
    # Discovery queries /api/tags for the catalog and /api/show per model for
    # capabilities, which is strictly better than a hand-written list -- it
    # picks up the real context windows (262k on the qwen3.5 line, 131k on
    # deepseek-r1, 16k on deepseek-coder), the `vision` capability that makes
    # the qwen3.5 models accept image attachments, and the `tools` capability
    # that deepseek-coder:6.7b lacks. Verified with
    # `openclaw models list --provider ollama`.
    #
    # Adding a non-empty `models` array here would switch openclaw to that
    # array and disable discovery entirely, so leave it out unless a model has
    # to be described in a way /api/show cannot express.
    #
    # Fields that only work on the explicit path are likewise omitted: on the
    # discovery path openclaw returns the *discovered* provider and does not
    # merge this block into it, so `timeoutSeconds` or per-model `params`
    # (e.g. keep_alive) would be silently dropped rather than applied. They
    # need the explicit `models` array to take effect.
    models.providers.ollama = {
      # No /v1 suffix. That path selects openclaw's OpenAI-compatible mode,
      # where upstream documents tool calling as unreliable -- models emit raw
      # tool-call JSON as assistant text instead. The bare origin selects the
      # native /api/chat API, which is what `api = "ollama"` below pins.
      baseUrl = "http://127.0.0.1:11434";
      # A documented non-secret marker, not a credential: the ollama plugin
      # lists "ollama-local" in its nonSecretAuthMarkers, and openclaw accepts
      # it for loopback/LAN/.local hosts instead of reporting a missing key.
      # Safe in the world-readable store for the same reason -- ollama itself
      # is unauthenticated on loopback, so there is nothing here to leak.
      apiKey = "ollama-local";
      # Redundant against the built-in provider default, but stated so a future
      # upstream change of default cannot quietly move this onto the /v1 path.
      api = "ollama";
    };

    agents.defaults = {
      inherit workspace;
      model.primary = "cliproxy/gpt-5.5";
    };

    # `/pair qr` has to embed a URL the phone can actually reach. The gateway
    # only knows it is bound to 127.0.0.1, so without this it refuses outright:
    #   Gateway is only bound to loopback. Set gateway.bind=lan, enable
    #   tailscale serve, or configure plugins.entries.device-pair.config.publicUrl
    #
    # publicUrl is the right one of those three for a reverse-proxied setup --
    # it tells the pairing plugin what the outside world calls this gateway
    # while leaving the socket on loopback. The alternatives are both wrong
    # here:
    #
    #   - gateway.bind = "lan" puts the raw gateway on the LAN, bypassing
    #     nginx's TLS and the tailnet/LAN allowlist that is the only thing
    #     restricting access to an agent with shell access.
    #   - `tailscale serve` would break unrelated services. Per ./goose.nix, it
    #     intercepts its configured ports inside tailscaled's netstack ahead of
    #     the host socket and hard-fails any SNI that is not the node's
    #     *.ts.net name -- so claiming 443 would take out every other vhost on
    #     romeo (jellyfin, immich, nextcloud, ...).
    #
    # Reachability of this name is a DNS question, not an nginx one: it resolves
    # to 192.168.3.7 on the LAN via unbound's h.b.nel.family redirect zone, but
    # a roaming client follows the *.h.b.nel.family wildcard to the public A
    # record and lands in `deny all`. Off-LAN pairing needs the explicit CNAME
    # to romeo.b.nel.family that ./goose.nix documents.
    #
    # No `enabled = true` needed: device-pair ships enabledByDefault and the
    # gateway loads it at startup. Running `openclaw qr` from a shell prints
    # "plugin disabled (bundled (disabled by default)) but config is present" --
    # that is the CLI's own activation context talking, not the gateway's, and
    # the QR renders anyway. publicUrl is the plugin's only accepted key
    # (configSchema sets additionalProperties: false).
    plugins.entries.device-pair.config.publicUrl = "https://${domain}";
  };
in {
  # Authenticates every Control UI / CLI client to the gateway. Generated rather
  # than hand-written so it never exists outside agenix.
  age.secrets.openclaw-gateway-token = {
    rekeyFile = ./secrets/openclaw_gateway_token.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
    # Nobody ever types this value, so without a Bitwarden copy the only ways to
    # read it are decrypting the repo secret or catting /run/agenix on romeo.
    #
    # `username` is what makes this a *login* item rather than a secure note,
    # and only login items carry URIs -- dropping it would silently cost the
    # domain match this exists for. There is no real username to record: the
    # Control UI prompts for a bearer token, so "token" names what the password
    # field actually holds.
    bitwarden = {
      name = "OpenClaw Gateway Token";
      username = "token";
      uris = {
        uri = "https://${domain}";
        matchType = "host";
      };
      notes = "Gateway auth token for OpenClaw on romeo-2 (gateway.auth.token). Paste it into the Control UI at https://${domain} on first connect from each browser; the device is remembered afterwards. Leave the password field empty -- no gateway password is configured. Rotating it means regenerating the agenix secret and rebuilding, since Nix mode forbids the gateway editing its own config.";
    };
  };

  # CLI_PROXY_API_KEY is the *proxy's* key, not a real OpenAI key -- it only
  # authenticates a localhost client to cli-proxy-api, which holds the actual
  # ChatGPT OAuth credential. Same secret librechat.nix and goose.nix present.
  age-template.files.openclaw-env = {
    vars = {
      GATEWAY_TOKEN = config.age.secrets.openclaw-gateway-token.path;
      PROXY_KEY = config.age.secrets.cli-proxy-api-key.path;
    };
    owner = "openclaw";
    group = "openclaw";
    mode = "0400";
    content = ''
      OPENCLAW_GATEWAY_TOKEN=$GATEWAY_TOKEN
      CLI_PROXY_API_KEY=$PROXY_KEY
    '';
  };

  users.users.openclaw = {
    isSystemUser = true;
    group = "openclaw";
    home = stateDir;
  };
  users.groups.openclaw = {};

  # On PATH so `sudo -u openclaw openclaw doctor` / `openclaw gateway status`
  # inspect exactly the config the daemon runs.
  environment.systemPackages = [openclaw];

  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 openclaw openclaw -"
    "d ${workspace} 0750 openclaw openclaw -"
  ];

  systemd.services.openclaw = {
    description = "OpenClaw gateway";
    wantedBy = ["multi-user.target"];
    # ollama.service is ordered but not required: the gateway starts fine
    # without it and simply discovers an empty ollama catalog, which would
    # leave `ollama/*` refs unresolvable until something re-triggers discovery.
    # Ordering after it makes the catalog populated on a clean boot.
    wants = ["network-online.target" "cli-proxy-api.service" "ollama.service"];
    after = ["network-online.target" "cli-proxy-api.service" "ollama.service"];

    environment = {
      OPENCLAW_NIX_MODE = "1";
      OPENCLAW_CONFIG_PATH = openclawConfig;
      OPENCLAW_STATE_DIR = stateDir;
      OPENCLAW_WORKSPACE_DIR = workspace;
      HOME = stateDir;

      # Required, and not obviously so: the `apiKey = "ollama-local"` in the
      # provider block above is enough for catalog *discovery*, but the agent
      # resolves the credential for an actual turn out of its own auth store
      # and falls back to this env var, not to provider config. Without it the
      # models list fine and every turn dies with
      #   No API key found for provider "ollama". Auth store:
      #   /var/lib/openclaw/agents/main/agent/openclaw-agent.sqlite
      # Not a secret -- see the marker note on models.providers.ollama.apiKey.
      #
      # Side effect worth knowing: the bundled ollama plugin declares this same
      # env var for its `ollama-cloud` provider, so setting it also makes four
      # hosted models (kimi-k2.5, minimax-m2.7, glm-5.1, glm-5.2) appear in the
      # catalog as authenticated. They are not: ollama.com rejects the local
      # marker, so selecting one fails the turn rather than silently shipping a
      # prompt off-box. Cosmetic noise in the picker, nothing more.
      OLLAMA_API_KEY = "ollama-local";
    };

    # The agent shells out; give it the same modest toolset goose.nix does
    # rather than leaving it with a bare PATH.
    path = [pkgs.bash pkgs.coreutils pkgs.git pkgs.ripgrep pkgs.jq pkgs.curl];

    serviceConfig = {
      ExecStart = "${lib.getExe openclaw} gateway --port ${toString port}";
      User = "openclaw";
      Group = "openclaw";
      StateDirectory = "openclaw";
      StateDirectoryMode = "0700";
      WorkingDirectory = workspace;
      EnvironmentFile = config.age-template.files.openclaw-env.path;
      Restart = "on-failure";
      RestartSec = "10s";

      # Deliberately moderate hardening, matching goose.nix: this thing exists
      # to spawn shell commands and plugin subprocesses, so the aggressive
      # filters used on cli-proxy-api (MemoryDenyWriteExecute, ~@privileged)
      # would break it. What is kept still blocks privilege escalation and
      # confines writes to the state dir and PrivateTmp.
      #
      # This sandbox is the only thing bounding a prompt-injected agent -- see
      # the knownVulnerabilities note above. ProtectSystem only blocks *writes*,
      # so the agent can still read /mnt/vault and friends and relay what it
      # reads to the model API. goose.nix documents why InaccessiblePaths= was
      # rejected there (it masks mounts with tmpfs and corrupts `df` output);
      # the same tradeoff applies here.
      NoNewPrivileges = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      SystemCallArchitectures = "native";
      RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
    };
  };

  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      # The Control UI drives the gateway over a long-lived WebSocket and an
      # agent turn can sit quiet for minutes while a tool runs, so the default
      # 60s read timeout would drop it mid-task.
      proxyWebsockets = true;
      extraConfig = ''
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
      '';
    };
  };
}
