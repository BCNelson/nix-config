{ config, lib, pkgs, ... }:
let
  cfg = config.services.config-merge;

  formatFor = instance:
    if instance.format != null then instance.format
    else if lib.hasSuffix ".json" instance.live then "json"
    else "toml";

  generatorFor = format:
    if format == "json" then (pkgs.formats.json { }).generate
    else (pkgs.formats.toml { }).generate;

  # A runtime key the base also declares can never apply: declarative always
  # wins. Wildcard patterns cannot be resolved statically, so they are skipped.
  pinnedRuntimeKeys = name: instance:
    let
      isPinned = key:
        let path = lib.splitString "." key;
        in !(lib.elem "*" path) && lib.hasAttrByPath path instance.settings;
    in
    lib.optional (instance.preserveUnknown && instance.runtimeKeys != [ ])
      "services.config-merge.${name}: runtimeKeys is ignored when preserveUnknown is set - every undeclared key is carried over already."
    ++ lib.optionals (!instance.preserveUnknown) (
      map (
        key:
        "services.config-merge.${name}: runtimeKeys entry \"${key}\" is also declared in settings, so it is a no-op - the declarative value always wins. Remove it from one or the other."
      ) (lib.filter isPinned instance.runtimeKeys)
    );

  mkService = name: instance:
    let
      format = formatFor instance;

      # The base never lands in the application's config directory: the daemon
      # reads it straight out of the store. Only the runtime overlay needs a
      # writable home, and it goes to state rather than next to the live file.
      baseFile = generatorFor format "config-merge-${name}-base" instance.settings;
      runtimeFile = "${config.xdg.stateHome}/config-merge/${name}.runtime.${format}";
    in
    {
      Unit = {
        Description = "config-merge daemon for ${name}";
      };

      Service = {
        ExecStart = lib.escapeShellArgs (
          [
            (lib.getExe pkgs.config-merge)
            "--base"
            "${baseFile}"
            "--runtime"
            runtimeFile
            "--live"
            instance.live
            "--format"
            format
            "--interval"
            instance.interval
          ]
          ++ lib.optional instance.preserveUnknown "--preserve-unknown"
          ++ lib.optionals (instance.onChange != "") [ "--on-change" instance.onChange ]
          ++ lib.concatMap (key: [ "--runtime-key" key ]) instance.runtimeKeys
        );
        Restart = "on-failure";
        RestartSec = 2;
      };

      Install = {
        WantedBy = [ "default.target" ];
      };
    };
in
{
  options.services.config-merge = lib.mkOption {
    default = { };
    description = ''
      Applications that persist their own settings cannot read their config from
      a read-only store symlink. Each instance here hands the live config file to
      a config-merge daemon, which renders {option}`settings` into it, harvests
      the application's own writes to {option}`runtimeKeys` into an overlay, and
      replays that overlay on every render. Keys named by {option}`settings`
      always win, so Nix stays the source of truth for everything it declares.
    '';
    type = lib.types.attrsOf (
      lib.types.submodule {
        options = {
          settings = lib.mkOption {
            type = lib.types.attrsOf lib.types.anything;
            default = { };
            description = "Declarative base config, rendered into the live file.";
          };

          live = lib.mkOption {
            type = lib.types.str;
            example = "\${config.xdg.configHome}/codex/config.toml";
            description = "Absolute path to the live config file the application reads and writes.";
          };

          runtimeKeys = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            example = [ "projects.*.trust_level" ];
            description = ''
              Dotted key paths the application owns at runtime, where `*` matches
              any key at that level. Only these are carried across renders;
              anything else the application writes is discarded. Ignored when
              {option}`preserveUnknown` is set.
            '';
          };

          preserveUnknown = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              Carry over every key {option}`settings` does not declare, rather
              than only the {option}`runtimeKeys` allowlist. Use this when the
              application persists settings you do not want to enumerate -
              declared keys still win, so this only widens what survives, never
              what Nix controls.

              The trade-off is that the overlay retains whatever it harvested:
              removing a key from {option}`settings` later leaves the last
              runtime value frozen in place instead of reverting to the
              application's own default.
            '';
          };

          format = lib.mkOption {
            type = lib.types.nullOr (lib.types.enum [ "toml" "json" ]);
            default = null;
            description = "Config format. Inferred from the {option}`live` file extension when null.";
          };

          interval = lib.mkOption {
            type = lib.types.str;
            default = "60s";
            description = "How often the daemon reconciles the live file.";
          };

          onChange = lib.mkOption {
            type = lib.types.str;
            default = "";
            example = "herdr server reload-config";
            description = ''
              Shell command run after the live file is rewritten. Applications
              that cache their config at startup will not notice the daemon
              editing it underneath them; this is where you tell them to reload.
              Failures are logged, never fatal.
            '';
          };
        };
      }
    );
  };

  config = lib.mkIf (cfg != { }) {
    warnings = lib.concatLists (lib.mapAttrsToList pinnedRuntimeKeys cfg);

    systemd.user.services = lib.mapAttrs' (
      name: instance: lib.nameValuePair "config-merge-${name}" (mkService name instance)
    ) cfg;
  };
}
