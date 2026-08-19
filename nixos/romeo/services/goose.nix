{ config, pkgs, lib, ... }:
let
  port = 3284; # goose serve default
  domain = "goose.h.b.nel.family";
  stateDir = "/var/lib/goose";

  # Reaching this vhost from off-LAN was a DNS problem, not an nginx one. nginx
  # already answers on romeo's tailscale interface -- it binds 0.0.0.0, and a
  # tailnet peer arrives with its own 100.64/10 source address, which the
  # allowlist below accepts unchanged. What was missing is that
  # *.h.b.nel.family is a wildcard CNAME to the public A record, so a roaming
  # client egressed over its WAN and got denied.
  #
  # Fixed in DNS rather than here: goose.h.b.nel.family is an explicit CNAME to
  # romeo.b.nel.family (A 100.76.49.168, romeo's tailnet address). An exact name
  # beats the wildcard, so every other *.h.b.nel.family service is untouched.
  # LAN clients never follow it -- romeo's unbound has an "h.b.nel.family"
  # redirect local-zone answering 192.168.3.7 first (see ../unbound.nix).
  #
  # This holds only while nothing else claims 443 on the tailnet address.
  # `tailscale serve` intercepts its configured ports inside tailscaled's
  # netstack, ahead of the host socket, and hard-fails any SNI that is not the
  # node's *.ts.net name. ./ollama.nix used to hold 443 that way; its Serve unit
  # was removed rather than moved, since nothing ever consumed it.

  # goose resolves every path through XDG, not $HOME (verified against 1.45.0:
  # `goose info` follows XDG_CONFIG_HOME/XDG_DATA_HOME/XDG_STATE_HOME and
  # ignores a $HOME override). Everything therefore hangs off StateDirectory.
  xdgConfig = "${stateDir}/config";
  xdgData = "${stateDir}/data";
  xdgState = "${stateDir}/state";
  workspace = "${stateDir}/workspace";

  # Both providers are wired up at once rather than picking one: romeo already
  # pays for the ChatGPT subscription (through cli-proxy-api) and already runs
  # local models (ollama), and the two suit different jobs. config.yaml only
  # names the *default*; any recipe overrides it per-run through
  # settings.goose_provider / settings.goose_model, and an interactive session
  # overrides it with GOOSE_PROVIDER/GOOSE_MODEL.
  #
  # Model names must exist in cli-proxy-api's catalog -- it serves
  # gpt-5.4{,-mini}, gpt-5.5, gpt-5.6-{luna,sol,terra} and the codex models.
  # There is no plain "gpt-5". Same list librechat.nix documents.
  defaultProvider = "openai";
  defaultModel = "gpt-5.5";

  configYaml = (pkgs.formats.yaml { }).generate "goose-config.yaml" {
    GOOSE_PROVIDER = defaultProvider;
    GOOSE_MODEL = defaultModel;

    # "auto" lets the agent run its tools without an approval prompt. That is
    # mandatory for the unattended timers below -- there is nobody to approve --
    # but see the security note on the service unit: it means goose can run
    # arbitrary shell commands as the `goose` user.
    GOOSE_MODE = "auto";

    # Bound a runaway agent. Without this a looping recipe bills the
    # subscription (or pins the GPU) until someone notices.
    GOOSE_MAX_TURNS = 50;

    extensions.developer = {
      name = "developer";
      type = "builtin";
      enabled = true;
      timeout = 300;
    };
  };

  # Non-secret provider wiring, shared by the daemon and every scheduled agent.
  commonEnv = {
    XDG_CONFIG_HOME = xdgConfig;
    XDG_DATA_HOME = xdgData;
    XDG_STATE_HOME = xdgState;
    HOME = stateDir;

    # There is no keyring on a headless box; without this goose tries to reach
    # a D-Bus secret service and fails to resolve provider credentials.
    GOOSE_DISABLE_KEYRING = "1";

    # goose's "openai" provider pointed at cli-proxy-api. OPENAI_HOST is the
    # origin only; OPENAI_BASE_PATH is appended to it.
    OPENAI_HOST = "http://127.0.0.1:8317";
    OPENAI_BASE_PATH = "v1/chat/completions";

    OLLAMA_HOST = "http://127.0.0.1:11434";
  };

  # Every goose unit needs the same credentials, sandbox and provider env.
  mkGooseUnit = extra: lib.recursiveUpdate {
    environment = commonEnv;
    path = [ pkgs.bash pkgs.coreutils pkgs.git pkgs.ripgrep pkgs.jq pkgs.curl ];

    serviceConfig = {
      User = "goose";
      Group = "goose";
      StateDirectory = "goose";
      StateDirectoryMode = "0700";
      WorkingDirectory = workspace;
      EnvironmentFile = config.age-template.files.goose-env.path;

      # Deliberately moderate hardening. goose's entire purpose is spawning
      # shell commands and MCP subprocesses, so the aggressive filters used on
      # cli-proxy-api (MemoryDenyWriteExecute, ~@privileged) break it. What is
      # kept still prevents privilege escalation and keeps it out of /home and
      # the rest of the filesystem.
      #
      # This sandbox is the ONLY thing bounding a misbehaving agent, because
      # GOOSE_MODE is "auto" -- goose itself approves every tool call. That is
      # not theoretical: qwen3.5:9b, asked only to run `uname -n`, instead
      # started authoring markdown files in its working directory. Verified on
      # romeo that with these settings a root-equivalent process can write to
      # /var/lib/goose and its PrivateTmp and nothing else -- /etc, /root,
      # /home, /nix/store, /mnt/vault, /trove and /data/replaceable all refuse
      # writes.
      #
      # Known residual exposure: ProtectSystem only blocks *writes*. The agent
      # can still READ /mnt/vault and friends and, since it talks to a model
      # API, could relay what it reads. InaccessiblePaths= would close that but
      # masks each mount with a tmpfs, so `df` reports the tmpfs (24G) instead
      # of the real pool and the health digest below would silently report
      # false capacity numbers. Wrong data was judged worse than the read
      # exposure on a single-tenant home server; revisit if an agent is ever
      # pointed at an untrusted prompt source.
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
      RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
    };
  } extra;

  # Scheduled agents. Each entry becomes a recipe YAML in the store plus a
  # oneshot service and timer. Add an entry here rather than running
  # `goose schedule add`: goose's built-in scheduler keeps its jobs in mutable
  # state under XDG_DATA_HOME, which would drift from this repo.
  scheduledAgents = {
    romeo-health-digest = {
      onCalendar = "*-*-* 07:15:00";
      # Deliberately NOT ollama. Both providers were smoke-tested on romeo with
      # the same one-line prompt ("run uname -n, reply with the hostname"):
      # gpt-5.5 through cli-proxy-api did exactly that in 8s, while
      # qwen3.5:9b ignored the prompt entirely, wandered off writing unrelated
      # markdown guides, and had to be killed at the 180s timeout. The local
      # models connect and their tool calls do execute -- so `GOOSE_PROVIDER=ollama`
      # is a fine thing to reach for by hand -- but none of the ones loaded here
      # are strong enough to be trusted with an unattended agent loop.
      provider = "openai";
      model = "gpt-5.5";
      recipe = {
        version = "1.0.0";
        title = "romeo health digest";
        description = "Daily plain-language summary of romeo's system health.";
        instructions = ''
          You are reporting on a NixOS home server named romeo. Use the shell to
          gather facts. Do not attempt to fix anything, do not modify any file,
          and do not restart any unit -- this is a read-only report.
        '';
        prompt = ''
          Collect the following and write a short digest:
            - systemctl --failed --no-legend --no-pager
            - zpool status -x
            - df -h -x tmpfs -x devtmpfs
            - journalctl -p err -b --no-pager --since "24 hours ago" | tail -n 50

          Then answer, in at most 15 lines: what is broken, what is close to
          breaking (any filesystem over 85% full counts), and what is fine.
          Lead with the single most important line. Plain text, no markdown.
        '';
        settings = {
          goose_provider = "openai";
          goose_model = "gpt-5.5";
          temperature = 0.1;
          # Tight bound: this job should need a handful of tool calls. Anything
          # much past that means the agent has lost the plot.
          max_turns = 20;
        };
      };
    };
  };

  recipeFile = name: agent:
    (pkgs.formats.yaml { }).generate "goose-recipe-${name}.yaml" agent.recipe;
in
{
  # Authenticates ACP clients to `goose serve`. Without it the daemon refuses
  # to start unless --dangerously-unauthenticated is passed, which we do not do
  # even behind nginx.
  age.secrets.goose-server-secret-key = {
    rekeyFile = ./secrets/goose_server_secret_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  # OPENAI_API_KEY is the *proxy's* key, not a real OpenAI key -- it only
  # authenticates a localhost client to cli-proxy-api, which holds the actual
  # ChatGPT OAuth credential. Same secret librechat.nix presents.
  age-template.files.goose-env = {
    vars = {
      PROXY_KEY = config.age.secrets.cli-proxy-api-key.path;
      SERVER_SECRET = config.age.secrets.goose-server-secret-key.path;
    };
    owner = "goose";
    group = "goose";
    mode = "0400";
    content = ''
      OPENAI_API_KEY=$PROXY_KEY
      GOOSE_SERVER__SECRET_KEY=$SERVER_SECRET
    '';
  };

  users.users.goose = {
    isSystemUser = true;
    group = "goose";
    home = stateDir;
  };
  users.groups.goose = { };

  # goose is on PATH so `sudo -u goose goose session` and `goose doctor` work
  # for debugging the same config the daemon uses.
  environment.systemPackages = [ pkgs.goose ];

  # config.yaml is owned by Nix. `L+` replaces whatever is there on every boot,
  # so changing the defaults above is a rebuild, not a manual edit -- goose
  # itself never gets to persist provider changes back into it.
  systemd.tmpfiles.rules = [
    "d ${xdgConfig} 0700 goose goose -"
    "d ${xdgConfig}/goose 0700 goose goose -"
    "d ${xdgData} 0700 goose goose -"
    "d ${xdgState} 0700 goose goose -"
    "d ${workspace} 0750 goose goose -"
    "L+ ${xdgConfig}/goose/config.yaml - - - - ${configYaml}"
  ];

  # `goose serve` speaks ACP (Agent Client Protocol) over HTTP + WebSocket.
  # This is NOT a browser chat UI -- goose 1.45 has no `web` subcommand. It is
  # the endpoint a Goose Desktop or Zed client connects to in order to drive an
  # agent that runs *here*, on romeo, with romeo's filesystem and shell.
  systemd.services = {
    goose = mkGooseUnit {
    description = "goose ACP agent server";
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" "cli-proxy-api.service" "ollama.service" ];
    after = [ "network-online.target" "cli-proxy-api.service" "ollama.service" ];

    serviceConfig = {
      ExecStart = lib.concatStringsSep " " [
        "${pkgs.goose}/bin/goose serve"
        "--host 127.0.0.1"
        "--port ${toString port}"
        # `--allowed-origin` replaces the default loopback origins rather than
        # extending them (goose 1.45: "may be specified multiple times and
        # replaces the default loopback origins"), so every origin that reaches
        # this daemon has to be named here.
        #
        # file:// is the one that makes the desktop client work, and it is not
        # the origin the app appears to use. Goose Desktop's renderer loads from
        # file://...app.asar/.vite/renderer/main_window/index.html, and Chromium
        # derives a WebSocket handshake's Origin from the document, so the ACP
        # upgrade arrives carrying exactly `Origin: file://`.
        #
        # Its main process does install
        #
        #   session.defaultSession.webRequest.onBeforeSendHeaders((d, cb) => {
        #     d.requestHeaders.Origin = "http://localhost:5173"; ... })
        #
        # -- a leftover from its Vite dev server -- but Electron's webRequest
        # hooks do not apply to WebSocket handshakes. Only the app's two plain
        # fetches (GET /status and a single non-upgrade GET /acp) carry that
        # value, and neither is origin-gated: both answered 200 and 406 while
        # localhost:5173 was not allowed. Allowing it therefore fixes nothing,
        # which is how 366c2eb deployed cleanly and changed nothing.
        #
        # Captured from goose-desktop 1.45.0 on redo-3 against a stand-in that
        # logs headers and completes the upgrade, then confirmed against a
        # throwaway goosed on romeo: with file:// listed the real client holds
        # an open WebSocket, and an unlisted origin still gets 403.
        #
        # https://${domain} is kept for browser-based ACP clients, which do send
        # the vhost origin.
        "--allowed-origin https://${domain}"
        "--allowed-origin file://"
      ];
      Restart = "on-failure";
      RestartSec = "10s";
    };
    };
  } // lib.mapAttrs' (name: agent:
    lib.nameValuePair "goose-agent-${name}" (mkGooseUnit {
      description = "goose scheduled agent: ${name}";
      # Not wantedBy multi-user.target -- the timer is the only trigger.
      wants = [ "network-online.target" ];
      after = [ "network-online.target" "cli-proxy-api.service" "ollama.service" ];

      environment = commonEnv // {
        GOOSE_PROVIDER = agent.provider;
        GOOSE_MODEL = agent.model;
      };

      serviceConfig = {
        Type = "oneshot";
        ExecStart = lib.concatStringsSep " " [
          "${pkgs.goose}/bin/goose run"
          "--recipe ${recipeFile name agent}"
          # No session file: these runs are unattended and resuming them makes
          # no sense, and it keeps the sessions DB from growing without bound.
          "--no-session"
        ];
        # An agent that wedges on a tool call should die, not run until morning.
        TimeoutStartSec = "30min";
      };
    })
  ) scheduledAgents;

  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      # romeo.b.nel.family carries an AAAA too, so a client following the CNAME
      # can arrive over the tailnet's ULA range; without this it would be denied.
      allow fd7a:115c:a1e0::/48;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      # ACP runs long-lived WebSocket sessions; an agent turn can easily sit
      # quiet for minutes while a tool runs, so the default 60s read timeout
      # would drop it mid-task.
      proxyWebsockets = true;
      extraConfig = ''
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
      '';
    };
  };

  systemd.timers = lib.mapAttrs' (name: agent:
    lib.nameValuePair "goose-agent-${name}" {
      description = "Schedule for goose agent: ${name}";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = agent.onCalendar;
        Persistent = true;
        RandomizedDelaySec = "5m";
      };
    }
  ) scheduledAgents;
}
