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

  # nixpkgs marks openclaw insecure: it parses untrusted content with an LLM
  # while holding full access to the system, so prompt injection is a
  # design-level exposure rather than a fixable CVE. That risk is real, and the
  # systemd sandbox below is what actually bounds it -- this override only stops
  # the marker from blocking evaluation.
  #
  # Done here rather than via nixpkgs.config.permittedInsecurePackages for two
  # reasons. First, that option is inert on this host: flake-utils-plus builds
  # `pkgs` from the channel and assigns nixpkgs.pkgs, so module-level
  # nixpkgs.config never reaches the package set (../nixarr.nix has the same
  # latent problem). Setting it in flake.nix `channelsConfig` would work but
  # would permit the package on every host to fix one service on romeo. Second,
  # permittedInsecurePackages matches name-with-version, so it would need a
  # manual edit on every nixpkgs bump; this override tracks the version by
  # construction, which matters on a host that auto-updates.
  #
  # Do NOT reach for nixpkgs.config.allowInsecurePredicate as an alternative --
  # check-meta treats predicate and list as an if/else-if chain, so defining a
  # predicate silently disables permittedInsecurePackages elsewhere in the host.
  openclaw = pkgs.openclaw.overrideAttrs (o: {
    meta = o.meta // {knownVulnerabilities = [];};
  });

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
      # Catalog must match what cli-proxy-api actually serves. There is no plain
      # "gpt-5"; the proxy exposes gpt-5.4{,-mini}, gpt-5.5 and gpt-5.6-*.
      # Same list librechat.nix and goose.nix document.
      models = [
        {id = "gpt-5.5";}
        {id = "gpt-5.4";}
        {id = "gpt-5.4-mini";}
      ];
    };

    agents.defaults = {
      workspace = workspace;
      model.primary = "cliproxy/gpt-5.5";
    };
  };
in {
  # Authenticates every Control UI / CLI client to the gateway. Generated rather
  # than hand-written so it never exists outside agenix.
  age.secrets.openclaw-gateway-token = {
    rekeyFile = ./secrets/openclaw_gateway_token.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
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
    wants = ["network-online.target" "cli-proxy-api.service"];
    after = ["network-online.target" "cli-proxy-api.service"];

    environment = {
      OPENCLAW_NIX_MODE = "1";
      OPENCLAW_CONFIG_PATH = openclawConfig;
      OPENCLAW_STATE_DIR = stateDir;
      OPENCLAW_WORKSPACE_DIR = workspace;
      HOME = stateDir;
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
