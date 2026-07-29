{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.thinClientBuilder;

  thinClientBuild = pkgs.writeShellApplication {
    name = "thin-client-build";
    runtimeInputs = with pkgs; [
      coreutils
      git
      git-crypt
      gnugrep
      jq
      nix
    ];
    text = builtins.readFile ./thin-client-build.sh;
  };

in
{
  options = {
    services.bcnelson.thinClientBuilder = {
      enable = lib.mkEnableOption ''
        building system closures on behalf of hosts that cannot build their
        own, publishing them to the local binary cache and writing the
        manifests those hosts poll
      '';

      hosts = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "delta-1" ];
        description = "Hostnames to build closures for.";
      };

      configPath = lib.mkOption {
        type = lib.types.str;
        description = ''
          Checkout of this flake to build from. Must be git-crypt unlocked.
          Normally the same path the host's own auto-update builds from.
        '';
      };

      cacheDir = lib.mkOption {
        type = lib.types.str;
        default = "/var/public-nix-cache";
        description = ''
          Directory the signed NARs and narinfos are written to. Must be what
          the binary cache vhost serves as its document root.
        '';
      };

      manifestDir = lib.mkOption {
        type = lib.types.str;
        default = "/var/lib/thin-client-builder/manifests";
        description = "Directory the per-host manifests are written to.";
      };

      signingKeyFile = lib.mkOption {
        type = lib.types.str;
        description = ''
          Path to the nix secret signing key used to sign published closures,
          e.g. an agenix secret path. Its public half must be configured on
          every client as services.bcnelson.thinClient.cachePublicKey.
        '';
      };

      domain = lib.mkOption {
        type = lib.types.str;
        description = ''
          Binary cache vhost the manifests are served from, under
          /thin-clients/. Must match the cache the clients fetch from.
        '';
      };

      triggerAfter = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "auto-update.service" ];
        description = ''
          Units that trigger a rebuild as soon as they finish. The build is
          ordered after them and pulled in by them, so a fresh commit landing
          on this machine immediately produces fresh closures for the clients
          instead of waiting for the next timer tick.
        '';
      };

      refreshInterval = lib.mkOption {
        type = lib.types.str;
        default = "6h";
        description = ''
          Backstop timer interval. The trigger units above are the primary
          mechanism; this only catches the case where none of them ran.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {

    assertions = [
      {
        assertion = cfg.configPath != "";
        message = "services.bcnelson.thinClientBuilder.configPath must be set";
      }
      {
        assertion = cfg.signingKeyFile != "";
        message = "services.bcnelson.thinClientBuilder.signingKeyFile must be set";
      }
    ];

    systemd.tmpfiles.rules = [
      "d ${cfg.cacheDir} 0755 root root -"
      "d ${cfg.manifestDir} 0755 root root -"
    ];

    services.nginx.virtualHosts.${cfg.domain}.locations."/thin-clients/" = {
      alias = "${cfg.manifestDir}/";
      extraConfig = ''
        default_type application/json;
        # Manifests are mutable pointers, unlike every other file this vhost
        # serves. The cache's `expires max` lives in its own `location /` and so
        # does not reach here, but say it outright rather than depend on that:
        # a cached manifest pins a client to a stale closure.
        add_header Cache-Control "no-store" always;
      '';
    };

    systemd.services.thin-client-build = {
      enable = true;
      description = "Build and publish system closures for thin clients";
      after = [ "network-online.target" ] ++ cfg.triggerAfter;
      wants = [ "network-online.target" ];
      # Pulled in by the trigger units rather than pulling them in: this must
      # never be the reason auto-update runs, only a consequence of it having
      # run. Ordering above makes "after it finishes", since those are oneshots.
      wantedBy = cfg.triggerAfter;
      environment = {
        CONFIG_PATH = cfg.configPath;
        CACHE_DIR = cfg.cacheDir;
        MANIFEST_DIR = cfg.manifestDir;
        SIGNING_KEY_FILE = cfg.signingKeyFile;
        THIN_HOSTS = lib.concatStringsSep " " cfg.hosts;
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${thinClientBuild}/bin/thin-client-build";
        TimeoutStartSec = "6h";
      } // lib.optionalAttrs config.services.bcnelson.autoUpdate.enable {
        # Share the auto-update slice so a burst of thin client builds is held
        # to the same CPU budget as the host's own rebuild and cannot starve the
        # services this machine actually runs. Only when that slice exists --
        # the autoUpdate module is what defines it.
        Slice = "system-autoupdate.slice";
      };
      restartIfChanged = false;
    };

    systemd.timers.thin-client-build = {
      enable = true;
      timerConfig = {
        OnBootSec = "20min";
        OnUnitActiveSec = cfg.refreshInterval;
        Persistent = true;
      };
      wantedBy = [ "timers.target" ];
    };
  };
}
