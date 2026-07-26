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

in
{
  options = {
    services.bcnelson.closurePublisher = {
      enable = lib.mkEnableOption ''
        building NixOS system closures for other hosts and publishing them to
        a local binary cache, so those hosts can install and update by
        downloading a pre-built closure instead of evaluating and building it
        themselves.

        Which hosts are published is derived, not configured: every
        nixosConfiguration with services.bcnelson.remoteUpdate enabled
      '';

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

      runAfter = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "auto-update.service" ];
        description = ''
          Publish whenever these units run, ordered after they finish, in
          addition to the timer.

          Point this at whatever maintains the checkout at
          <option>flakePath</option> — <literal>auto-update.service</literal>
          — and publishing inherits its schedule *and* its triggers: a pushed
          ntfy refresh starts autoUpdate, autoUpdate pulls, and a publish
          follows on the same fresh checkout. There is no second subscription
          to keep in sync and no window in which this builds a stale tree.

          Implemented as <literal>WantedBy=</literal> plus
          <literal>After=</literal> on this service, so the listed units are
          not modified. Publishing still happens if they fail, which is what
          you want: a rebuild that broke on *this* host says nothing about
          whether the thin clients' closures are worth republishing.
        '';
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
      # Pulled into the same transaction as each runAfter unit and ordered
      # behind it, so the checkout is already up to date when we build.
      wantedBy = cfg.runAfter;
      after = cfg.runAfter;
      environment = {
        FLAKE_PATH = cfg.flakePath;
        CACHE_DIR = cfg.cacheDir;
        MANIFEST_SUBDIR = cfg.manifestSubdir;
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
