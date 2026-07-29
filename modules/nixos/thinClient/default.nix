{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.thinClient;

  thinClientUpdate = pkgs.writeShellApplication {
    name = "thin-client-update";
    runtimeInputs = with pkgs; [
      coreutils
      curl
      gnugrep
      jq
      nix
      systemd
    ];
    text = builtins.readFile ./thin-client-update.sh;
  };

in
{
  options = {
    services.bcnelson.thinClient = {
      enable = lib.mkEnableOption ''
        pull-only system updates for hosts that cannot build or evaluate their
        own closure. The host polls a manifest published by the builder and
        fetches the prebuilt system closure from the binary cache
      '';

      cacheUrl = lib.mkOption {
        type = lib.types.str;
        default = "https://nixcache.nel.family";
        description = "Binary cache the prebuilt system closure is fetched from.";
      };

      manifestUrl = lib.mkOption {
        type = lib.types.str;
        default = "${cfg.cacheUrl}/thin-clients/${config.networking.hostName}.json";
        defaultText = lib.literalExpression ''"''${cacheUrl}/thin-clients/''${networking.hostName}.json"'';
        description = "URL of this host's build manifest.";
      };

      cachePublicKey = lib.mkOption {
        type = lib.types.str;
        default = "";
        example = "nixcache.nel.family-1:AbCdEf...";
        description = ''
          Public half of the key the builder signs closures with. Added to
          nix.settings.trusted-public-keys. Required: without it the client
          cannot verify anything the builder produced.
        '';
      };

      pollInterval = lib.mkOption {
        type = lib.types.str;
        default = "15m";
        description = "How often to poll the manifest for a new closure.";
      };

      reboot = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Reboot automatically when the new closure needs one.";
      };

      maxRetries = lib.mkOption {
        type = lib.types.int;
        default = 3;
        description = ''
          Attempts per published closure before giving up. Failure is still
          reported to the health check after the limit is reached, and a newly
          published closure resets the counter.
        '';
      };

      healthCheck = {
        enable = lib.mkEnableOption "Report update runs to a health check endpoint";
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
    };
  };

  config = lib.mkIf cfg.enable {

    assertions = [
      {
        assertion = cfg.cachePublicKey != "";
        message = ''
          services.bcnelson.thinClient.cachePublicKey must be set, otherwise
          this host has no way to verify the closures it is asked to run.
          Generate the builder's signing key with `just generate-secrets`, then
          `just rekey`, and commit both halves -- see docs/thin-clients.md
          ("Cache signing key").
        '';
      }
      {
        assertion = !cfg.healthCheck.enable || (cfg.healthCheck.url != "" && cfg.healthCheck.uuidFile != "");
        message = "Health check URL and UUID file must be set if the thin client health check is enabled";
      }
    ];

    nix.settings = {
      substituters = lib.mkBefore [ cfg.cacheUrl ];
      trusted-public-keys = [ cfg.cachePublicKey ];
      # Nothing on this host should ever compile anything -- there is not enough
      # RAM for it and a build here means the closure was not published
      # properly. Fail loudly instead of thrashing into the OOM killer.
      max-jobs = 0;
      # These only exist to keep build inputs alive for local development, which
      # never happens here, and they cost real disk on a thin client.
      keep-outputs = lib.mkForce false;
      keep-derivations = lib.mkForce false;
    };

    systemd.timers.thin-client-update = {
      enable = true;
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = cfg.pollInterval;
        Persistent = true;
      };
      wantedBy = [ "timers.target" ];
    };

    systemd.services.thin-client-update = {
      enable = true;
      description = "Fetch and activate the system closure built for this host";
      # The timer fires shortly after boot, which can beat DNS coming up.
      wants = [ "network-online.target" ];
      after = [ "network-online.target" "nix-daemon.socket" ];
      environment = {
        MANIFEST_URL = cfg.manifestUrl;
        CACHE_URL = cfg.cacheUrl;
        REBOOT = if cfg.reboot then "true" else "false";
        MAX_RETRIES = toString cfg.maxRetries;
        HEALTHCHECK_URL = if cfg.healthCheck.enable then cfg.healthCheck.url else "";
        HEALTHCHECK_UUID_FILE = if cfg.healthCheck.enable then cfg.healthCheck.uuidFile else "";
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${thinClientUpdate}/bin/thin-client-update";
        # Fetching a full system closure over a slow link on a slow box.
        TimeoutStartSec = "3h";
        StateDirectory = "thin-client-update";
      };
      # This unit activates the new system; restarting it mid-switch as part of
      # that very switch would kill the switch.
      restartIfChanged = false;
    };
  };
}
