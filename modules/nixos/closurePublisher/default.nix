{ config, lib, pkgs, ... }:

let
  cfg = config.services.bcnelson.closurePublisher;

  publishScript = pkgs.writeShellApplication {
    name = "publish-closures";
    runtimeInputs = with pkgs; [
      coreutils
      findutils
      gawk
      gnugrep
      gnused
      git
      nix
    ];
    text = builtins.readFile ./publish-closures.sh;
  };

  # What an ntfy refresh message actually runs. The wait matters: this builds
  # from a checkout somebody else maintains (autoUpdate), and both subscribers
  # wake on the same message, so publishing immediately would race the pull and
  # republish the commit we already had.
  refreshHandler = pkgs.writeShellApplication {
    name = "closure-publisher-refresh";
    runtimeInputs = [ pkgs.systemd ];
    text = ''
      ${lib.concatMapStrings (unit: ''
        echo "Waiting for ${unit} to settle"
        systemctl start --wait ${lib.escapeShellArg unit} || true
      '') cfg.ntfy-refresh.afterUnits}
      echo "Publishing closures"
      systemctl start closure-publisher.service
    '';
  };

  ntfyRefreshClient = pkgs.writeShellApplication {
    name = "closure-publisher-ntfy-client";
    runtimeInputs = with pkgs; [ coreutils ntfy-sh ];
    text = builtins.readFile ./ntfy-refresh-client.sh;
  };
in
{
  options = {
    services.bcnelson.closurePublisher = {
      enable = lib.mkEnableOption ''
        building NixOS system closures for other hosts and publishing them to a
        local binary cache, so those hosts can update by downloading a
        pre-built closure instead of evaluating and building it themselves
      '';

      hosts = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "delta-1" ];
        description = ''
          nixosConfigurations attribute names to build and publish. Each one
          gets a manifest at <literal>''${manifestSubdir}/&lt;host&gt;</literal>
          under the cache directory containing its current toplevel store path.
        '';
      };

      flakePath = lib.mkOption {
        type = lib.types.str;
        default = "/config";
        description = "Path to the checkout of this flake to build from.";
      };

      cacheDir = lib.mkOption {
        type = lib.types.str;
        default = "/var/public-nix-cache";
        description = ''
          Directory served as a binary cache. Defaults to the directory
          <literal>services.bcnelson.binary-cache-proxy</literal> serves, so
          published closures are reachable at that proxy's domain.
        '';
      };

      extraAttributes = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "diskoScript" ];
        description = ''
          Additional <literal>system.build.&lt;attr&gt;</literal> outputs to
          build and publish alongside the toplevel, each getting its own
          manifest at <literal>&lt;host&gt;.&lt;attr&gt;</literal>.

          <literal>diskoScript</literal> is the useful one: an installer booted
          on the target can fetch and run it to partition the disk without
          evaluating this flake. Attributes that do not exist for a given host
          are skipped with a warning rather than failing the run.
        '';
      };

      manifestSubdir = lib.mkOption {
        type = lib.types.str;
        default = "system";
        description = "Subdirectory of cacheDir holding the per-host manifests.";
      };

      signingKeyFile = lib.mkOption {
        type = lib.types.str;
        default = "/var/lib/closure-publisher/cache-priv-key.pem";
        description = ''
          Private binary cache signing key. Generated on first run if absent;
          the matching public key is published at
          <literal>''${cacheDir}/nix-cache-pubkey</literal> so clients can pin
          it (see <literal>just cache-key</literal>).
        '';
      };

      signingKeyName = lib.mkOption {
        type = lib.types.str;
        default = "${config.networking.hostName}-closures-1";
        defaultText = lib.literalExpression ''"''${config.networking.hostName}-closures-1"'';
        description = ''
          Name embedded in the signing key. Only meaningful when the key is
          first generated; changing it afterwards has no effect.
        '';
      };

      interval = lib.mkOption {
        type = lib.types.str;
        default = "30m";
        description = "How often to rebuild and republish the closures.";
      };

      retentionDays = lib.mkOption {
        type = lib.types.int;
        default = 7;
        description = ''
          Prune cache entries no published closure references any more, once
          they are at least this many days old. Set to a negative number to
          disable pruning entirely.
        '';
      };

      ntfy-refresh = {
        enable = lib.mkEnableOption ''
          republishing on a pushed ntfy message as well as on the timer, the
          same way services.bcnelson.autoUpdate does
        '';

        topicFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "File containing the ntfy topic to subscribe to.";
        };

        afterUnits = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          example = [ "auto-update.service" ];
          description = ''
            Units to run to completion before publishing, when publishing was
            triggered by a refresh message. This is how the checkout at
            <option>flakePath</option> gets brought up to date first — without
            it, both subscribers wake on the same message and this one can win,
            republishing the commit that was already published.

            These are also added to the service's <literal>After=</literal>.
            Units with <literal>RemainAfterExit=yes</literal> are not usable
            here: waiting on one would block forever.
          '';
        };
      };

      nginxVirtualHost = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "nixcache.nel.family";
        description = ''
          When set, adds an nginx location for the manifest directory on this
          virtual host that disables HTTP caching. The binary cache itself is
          content addressed and served with <literal>expires max</literal>,
          but the manifests are mutable pointers and must never be cached.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.hosts != [ ];
        message = "services.bcnelson.closurePublisher.hosts must list at least one host";
      }
      {
        assertion = !cfg.ntfy-refresh.enable || cfg.ntfy-refresh.topicFile != "";
        message = "services.bcnelson.closurePublisher.ntfy-refresh needs topicFile";
      }
    ];

    # Publishing is background work on a machine that is also serving. Keep it
    # off the critical path the same way autoUpdate does.
    systemd.slices.system-closure-publisher = {
      enable = true;
      wantedBy = [ "system.slice" ];
      sliceConfig = {
        CPUAccounting = true;
        CPUWeight = 10;
        IOWeight = 20;
      };
    };

    systemd.timers.closure-publisher = {
      enable = true;
      timerConfig = {
        OnBootSec = "10min";
        OnUnitActiveSec = cfg.interval;
        Persistent = true;
      };
      wantedBy = [ "timers.target" ];
    };

    systemd.services.closure-publisher = {
      description = "Build and publish NixOS system closures for remote hosts";
      # Covers the case where both jobs are queued together (at boot, or when
      # two timers coincide); the refresh handler covers the pushed case.
      after = cfg.ntfy-refresh.afterUnits;
      environment = {
        FLAKE_PATH = cfg.flakePath;
        CACHE_DIR = cfg.cacheDir;
        MANIFEST_SUBDIR = cfg.manifestSubdir;
        TARGET_HOSTS = lib.concatStringsSep " " cfg.hosts;
        EXTRA_ATTRIBUTES = lib.concatStringsSep " " cfg.extraAttributes;
        SIGNING_KEY_FILE = cfg.signingKeyFile;
        SIGNING_KEY_NAME = cfg.signingKeyName;
        RETENTION_DAYS = toString cfg.retentionDays;
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${publishScript}/bin/publish-closures";
        TimeoutStartSec = "6h";
        Slice = "system-closure-publisher.slice";
      };
      restartIfChanged = false;
    };

    systemd.services.closure-publisher-ntfy-client = lib.mkIf cfg.ntfy-refresh.enable {
      description = "Republish closures when a refresh is pushed over ntfy";
      enable = true;
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = {
        NTFY_REFRESH_TOPIC_FILE = cfg.ntfy-refresh.topicFile;
        REFRESH_COMMAND = "${refreshHandler}/bin/closure-publisher-refresh";
      };
      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = "${ntfyRefreshClient}/bin/closure-publisher-ntfy-client";
        Restart = "always";
        RestartSec = 30;
        Slice = "system-closure-publisher.slice";
      };
      restartIfChanged = true;
    };

    services.nginx.virtualHosts = lib.mkIf (cfg.nginxVirtualHost != null) {
      "${cfg.nginxVirtualHost}" = {
        locations."/${cfg.manifestSubdir}/" = {
          root = cfg.cacheDir;
          recommendedProxySettings = false;
          extraConfig = ''
            expires -1;
            add_header Cache-Control "no-store" always;
          '';
        };
      };
    };
  };
}
