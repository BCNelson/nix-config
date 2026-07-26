{ config, lib, pkgs, ... }:

let
  cfg = config.services.bcnelson.remoteUpdate;

  remoteUpdateScript = pkgs.writeShellApplication {
    name = "remote-update";
    runtimeInputs = with pkgs; [
      coreutils
      curl
      gnugrep
      hostname
      nix
      systemd
    ];
    text = builtins.readFile ./remote-update.sh;
  };

  # What a refresh message actually does. Not "check now": the builder still
  # has to pull, evaluate and build this host's closure before there is
  # anything new to fetch, so an immediate check just re-reads the old path.
  refreshHandler = pkgs.writeShellApplication {
    name = "remote-update-refresh";
    runtimeInputs = [ pkgs.systemd ];
    text = ''
      # Replace any check already pending, so a burst of pushes collapses into
      # one run ${cfg.ntfy-refresh.delay} after the last of them.
      systemctl stop remote-update-delayed.timer 2>/dev/null || true
      systemctl stop remote-update-delayed.service 2>/dev/null || true
      echo "Scheduling a closure check in ${cfg.ntfy-refresh.delay}"
      systemd-run \
        --unit=remote-update-delayed \
        --on-active=${lib.escapeShellArg cfg.ntfy-refresh.delay} \
        --timer-property=AccuracySec=1min \
        systemctl start remote-update.service
    '';
  };

  ntfyRefreshClient = pkgs.writeShellApplication {
    name = "remote-update-ntfy-client";
    runtimeInputs = with pkgs; [ coreutils ntfy-sh ];
    text = builtins.readFile ./ntfy-refresh-client.sh;
  };
in
{
  options = {
    services.bcnelson.remoteUpdate = {
      enable = lib.mkEnableOption ''
        updating this host from a pre-built system closure published by another
        machine, instead of building it locally. Intended for hardware that
        cannot evaluate or build this flake — see
        services.bcnelson.closurePublisher for the other half
      '';

      manifestUrl = lib.mkOption {
        type = lib.types.str;
        example = "https://nixcache.nel.family/system/delta-1";
        description = ''
          URL of a plain text file whose first line is the store path of the
          system closure this host should be running.
        '';
      };

      cacheUrl = lib.mkOption {
        type = lib.types.str;
        example = "https://nixcache.nel.family";
        description = "Binary cache to copy the published closure from.";
      };

      checkSignatures = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Verify the closure's signatures against
          <literal>nix.settings.trusted-public-keys</literal>. Requires the
          publisher's cache key to be in
          <option>services.bcnelson.remoteUpdate.trustedPublicKeys</option>.

          When disabled, only this one fetch skips signature checking — the
          host's global <literal>require-sigs</literal> stays on — so the
          closure is trusted on the strength of the HTTPS connection to
          <option>cacheUrl</option> alone.
        '';
      };

      trustedPublicKeys = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "romeo-2-closures-1:XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX=" ];
        description = ''
          Binary cache public keys to trust, added to
          <literal>nix.settings.trusted-public-keys</literal>. Read the
          publisher's key with <literal>just cache-key</literal>.
        '';
      };

      substituters = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ cfg.cacheUrl ];
        defaultText = lib.literalExpression "[ config.services.bcnelson.remoteUpdate.cacheUrl ]";
        description = ''
          Substituters for this host, replacing the default. Pointing a thin
          client at the publisher's cache keeps it from reaching for
          cache.nixos.org directly; the publisher's cache proxies upstream
          anyway.
        '';
      };

      expectedHostname = lib.mkOption {
        type = lib.types.str;
        default = config.networking.hostName;
        defaultText = lib.literalExpression "config.networking.hostName";
        description = ''
          Refuse to activate a closure built for a different host. Set to the
          empty string to skip the check.
        '';
      };

      refreshInterval = lib.mkOption {
        type = lib.types.str;
        default = "6h";
        description = ''
          How often to poll the manifest. This is a backstop: with
          <option>ntfy-refresh</option> enabled, a push is what normally
          brings an update in, and polling only has to catch the cases a
          push missed (the host was off, the message was dropped, the
          builder was slow).
        '';
      };

      ntfy-refresh = {
        enable = lib.mkEnableOption ''
          checking for a new closure a while after a pushed ntfy message,
          rather than relying on the poll interval alone
        '';

        topicFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = ''
            File containing the ntfy topic to subscribe to. The same topic
            autoUpdate hosts listen on: the push that tells them to pull is
            also what eventually produces this host's new closure.
          '';
        };

        delay = lib.mkOption {
          type = lib.types.str;
          default = "30m";
          example = "45m";
          description = ''
            How long to wait after a refresh message before checking, as a
            systemd time span. The message means "there is a new commit", not
            "there is a new closure" — the builder has to pull, evaluate and
            build it first — so checking immediately would just re-read the
            path already running.

            If the builder regularly takes longer than this, the check finds
            nothing and the update waits for the next poll or push. Size it
            against how long a publish actually takes, not how impatient you
            feel.
          '';
        };
      };

      reboot = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Reboot when the new closure needs it (new kernel or initrd).";
      };

      maxRetries = lib.mkOption {
        type = lib.types.int;
        default = 3;
        description = ''
          Activation attempts per published closure before giving up. Failure
          is still reported to the health check after the limit is hit, until a
          new closure is published.
        '';
      };

      healthCheck = {
        enable = lib.mkEnableOption "health check pings";
        url = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Health check base URL.";
        };
        uuidFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "File containing the health check UUID.";
        };
      };

      ntfy = {
        enable = lib.mkEnableOption "ntfy notifications";
        topicFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "File containing the ntfy topic.";
        };
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !cfg.checkSignatures || cfg.trustedPublicKeys != [ ];
        message = ''
          services.bcnelson.remoteUpdate.checkSignatures is enabled but no
          trustedPublicKeys are set, so no published closure could ever be
          verified. Read the publisher's key with `just cache-key` and add it,
          or set checkSignatures = false.
        '';
      }
      {
        assertion = !cfg.healthCheck.enable || (cfg.healthCheck.url != "" && cfg.healthCheck.uuidFile != "");
        message = "services.bcnelson.remoteUpdate.healthCheck needs both url and uuidFile";
      }
      {
        assertion = !cfg.ntfy.enable || cfg.ntfy.topicFile != "";
        message = "services.bcnelson.remoteUpdate.ntfy needs topicFile";
      }
      {
        assertion = !cfg.ntfy-refresh.enable || cfg.ntfy-refresh.topicFile != "";
        message = "services.bcnelson.remoteUpdate.ntfy-refresh needs topicFile";
      }
    ];

    nix.settings = {
      substituters = cfg.substituters;
      # `extra-` so the publisher's key is added to nixpkgs' default rather
      # than replacing it, which would leave cache.nixos.org unverifiable.
      extra-trusted-public-keys = cfg.trustedPublicKeys;
    };

    systemd.timers.remote-update = {
      enable = true;
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = cfg.refreshInterval;
        Persistent = true;
      };
      wantedBy = [ "timers.target" ];
    };

    systemd.services.remote-update = {
      description = "Activate the system closure published for this host";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      environment = {
        MANIFEST_URL = cfg.manifestUrl;
        CACHE_URL = cfg.cacheUrl;
        CHECK_SIGNATURES = if cfg.checkSignatures then "true" else "false";
        EXPECTED_HOSTNAME = cfg.expectedHostname;
        REBOOT = if cfg.reboot then "true" else "false";
        MAX_RETRIES = toString cfg.maxRetries;
        HEALTHCHECK_URL = if cfg.healthCheck.enable then cfg.healthCheck.url else "";
        HEALTHCHECK_UUID_FILE = if cfg.healthCheck.enable then cfg.healthCheck.uuidFile else "";
        NTFY_TOPIC_FILE = if cfg.ntfy.enable then cfg.ntfy.topicFile else "";
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${remoteUpdateScript}/bin/remote-update";
        TimeoutStartSec = "2h";
        StateDirectory = "remote-update";
      };
      restartIfChanged = false;
    };

    systemd.services.remote-update-ntfy-client = lib.mkIf cfg.ntfy-refresh.enable {
      description = "Schedule a closure check when a refresh is pushed over ntfy";
      enable = true;
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = {
        NTFY_REFRESH_TOPIC_FILE = cfg.ntfy-refresh.topicFile;
        REFRESH_COMMAND = "${refreshHandler}/bin/remote-update-refresh";
      };
      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = "${ntfyRefreshClient}/bin/remote-update-ntfy-client";
        Restart = "always";
        RestartSec = 30;
      };
      restartIfChanged = true;
    };
  };
}
