# Upstream candidate: nix/modules/sysusers.nix
#
# Modelled on nix/modules/tmpfiles.nix, which exposes the same rules/settings/
# packages triple for systemd-tmpfiles.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    concatStrings
    concatStringsSep
    getLib
    literalExpression
    mapAttrsToList
    mkOption
    types
    ;

  cfg = config.systemd.sysusers;

  sysusersDocs = "https://www.freedesktop.org/software/systemd/man/sysusers.d";

  settingsOption = {
    description = ''
      Declare systemd-sysusers entries to create system users and groups.

      Unlike `services.userborn.enable`, systemd-sysusers is strictly additive:
      it does nothing if the named user or group already exists, and it never
      renames, renumbers, or removes an existing one, nor changes an existing
      group's members. That makes it a good fit for the distributions
      system-manager targets, where {file}`/etc/passwd` and {file}`/etc/group`
      are owned by the host distribution and only need a few extra entities --
      a group referenced by a udev rule, or a system user for a service.

      When system-manager should instead own the accounts it declares, use
      `services.userborn.enable`.

      Please see the upstream documentation for the exact semantics:
      <${sysusersDocs}>
    '';
    example = {
      "10-mypackage" = {
        plugdev.g = { };
        myservice.u = {
          id = "404";
          gecos = "My service";
        };
      };
    };
    default = { };
    type = types.attrsOf (
      types.attrsOf (
        types.attrsOf (
          types.submodule (
            { name, ... }:
            {
              options = {
                type = mkOption {
                  type = types.str;
                  default = name;
                  example = "g";
                  description = ''
                    The type of entity to create.

                    `u` creates a system user (and its group), `g` a group, `m`
                    adds a user to a group, and `r` reserves a UID/GID range.

                    Please see the upstream documentation for the available
                    types and more details:
                    <${sysusersDocs}>
                  '';
                };
                id = mkOption {
                  type = types.str;
                  default = "-";
                  example = "404";
                  description = ''
                    The numeric ID to request, the group name for `m` entries,
                    or the range for `r` entries.

                    If omitted or when set to `"-"`, systemd allocates an ID
                    from the system range. Prefer this: a fixed ID that the
                    host distribution has already handed to something else is
                    silently ignored and an ID is allocated anyway.
                  '';
                };
                gecos = mkOption {
                  type = types.str;
                  default = "-";
                  example = "My service";
                  description = ''
                    A short description of the user, placed in the GECOS field
                    of {file}`/etc/passwd`. Only used by `u` entries.
                  '';
                };
                home = mkOption {
                  type = types.str;
                  default = "-";
                  example = "/var/lib/my-service";
                  description = ''
                    The home directory of the user. Only used by `u` entries.

                    Note that systemd-sysusers does not create this directory;
                    use `systemd.tmpfiles` for that.
                  '';
                };
                shell = mkOption {
                  type = types.str;
                  default = "-";
                  example = "/usr/bin/bash";
                  description = ''
                    The login shell of the user. Only used by `u` entries.

                    If omitted or when set to `"-"`, a nologin shell is used.
                  '';
                };
              };
            }
          )
        )
      )
    );
  };

  # The first three columns are always emitted, because `type` and `name` are
  # mandatory and a bare two-column line is not accepted for every type.
  # Trailing unset columns are dropped so that e.g. an `m` entry renders as
  # "m alice wheel" rather than "m alice wheel - - -".
  stripTrailingUnset =
    fields: if fields != [ ] && lib.last fields == "-" then stripTrailingUnset (lib.init fields) else fields;

  quoteField = value: if value == "-" then "-" else ''"${value}"'';

  settingsEntryToLine =
    name: entry:
    let
      fields = [
        entry.type
        name
        entry.id
      ]
      ++ stripTrailingUnset [
        (quoteField entry.gecos)
        entry.home
        entry.shell
      ];
    in
    concatStringsSep " " fields + "\n";

  # generates a list of sysusers.d lines from the attrs (names) under
  # sysusers.settings.<file>
  namesToLines = mapAttrsToList (
    name: entryTypes: concatStrings (mapAttrsToList (_type: settingsEntryToLine name) entryTypes)
  );

  mkFileContent = names: concatStrings (namesToLines names);

  sysusersDir = pkgs.symlinkJoin {
    name = "sysusers.d";
    paths = map (p: p + "/lib/sysusers.d") cfg.packages;
    postBuild = ''
      for i in $(cat $pathsPath); do
        (test -d "$i" && test $(ls "$i"/*.conf | wc -l) -ge 1) || (
          echo "ERROR: The path '$i' from systemd.sysusers.packages contains no *.conf files."
          exit 1
        )
      done
    '';
  };

  hasEntries = cfg.rules != [ ] || cfg.settings != { };
in
{
  options = {
    systemd.sysusers.rules = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "g plugdev -" ];
      description = ''
        Lines declaring system users and groups to create. See
        {manpage}`sysusers.d(5)` for the exact format.

        See `systemd.sysusers.settings` for the trade-offs against
        `services.userborn.enable`.
      '';
    };

    systemd.sysusers.settings = mkOption settingsOption;

    systemd.sysusers.executable = mkOption {
      type = types.str;
      default = "${pkgs.systemd}/bin/systemd-sysusers";
      defaultText = literalExpression ''"''${pkgs.systemd}/bin/systemd-sysusers"'';
      example = "/usr/bin/systemd-sysusers";
      description = ''
        The {command}`systemd-sysusers` binary to run.

        Override this on SELinux distributions. `pkgs.systemd` is built with
        `withSelinux = false`, so its {command}`systemd-sysusers` cannot call
        {manpage}`setfscreatecon(3)` before creating the temporary file it
        renames over {file}`/etc/group`. The label then comes from whatever
        type transition the service's domain happens to carry, which is
        generally not the one {file}`file_contexts` specifies -- on Fedora's
        targeted policy it silently produces an unreadable {file}`/etc/group`
        and the host stops booting.

        Point this at `pkgs.systemd.override { withSelinux = true; }`, which
        sets the create context itself so the label is correct regardless of
        domain. Verify with `systemd-sysusers --version`: it reports `-SELINUX`
        for the stock build and `+SELINUX` once support is in. Note that
        systemd dlopens libselinux rather than linking it, so {command}`ldd`
        shows nothing either way and is not a usable check.

        The host's own {file}`/usr/bin/systemd-sysusers` is also built with
        libselinux, but it is only usable if `SELinuxContext` is left unset or
        names a domain holding `entrypoint` on its type -- typically `bin_t`,
        which the policy's user-management domain does not have.
      '';
    };

    systemd.sysusers.packages = mkOption {
      type = types.listOf types.package;
      default = [ ];
      example = literalExpression "[ pkgs.nginx ]";
      apply = map getLib;
      description = ''
        List of packages containing {command}`systemd-sysusers` configuration.

        All files ending in .conf found in
        {file}`«pkg»/lib/sysusers.d`
        will be included.
        If this folder does not exist or does not contain any files an error will be returned instead.

        If a {file}`lib` output is available, configuration is searched there and only there.
        If there is no {file}`lib` output it will fall back to {file}`out`
        and if that does not exist either, the default output will be used.
      '';
    };
  };

  config = {
    environment.etc = {
      "sysusers.d".source = sysusersDir;
    };

    systemd.sysusers.packages = [
      (pkgs.writeTextFile {
        name = "system-manager-sysusers.d";
        destination = "/lib/sysusers.d/00-system-manager.conf";
        text = ''
          # This file is created automatically and should not be modified.
          # Please change the option ‘systemd.sysusers.rules’ instead.

          ${concatStringsSep "\n" cfg.rules}
        '';
      })
    ]
    ++ (mapAttrsToList (
      name: names: pkgs.writeTextDir "lib/sysusers.d/${name}.conf" (mkFileContent names)
    ) cfg.settings);

    # systemd's own systemd-sysusers.service cannot be relied on to apply these.
    # It is gated on `ConditionNeedsUpdate=|/etc`, which fires when /usr is
    # newer than /etc -- i.e. after a package update -- and writing into
    # /etc/sysusers.d does not set that flag. Observed on Fedora 43: the unit
    # logs "skipped, no trigger condition checks were met" on a normal boot, so
    # entries would appear only whenever an unrelated distribution update
    # happened to touch /usr. Hence an explicit unit, which also means
    # activation does not have to wait for a reboot.
    #
    # Only defined when there is something to create, so that enabling
    # system-manager does not start running sysusers on hosts that never asked
    # for it.
    #
    # SYSUSERS_D pins the generated directory into the unit so that the unit
    # file itself changes whenever the entries do; system-manager restarts units
    # whose contents changed, which is what re-runs this. It is otherwise
    # unused -- systemd-sysusers is invoked without arguments so that it also
    # picks up vendor configuration.
    #
    # On SELinux-enforcing distributions this unit needs a domain that may write
    # shadow_t, or it gets as far as /etc/group and then fails with "Failed to
    # open /etc/gshadow: Permission denied". Set serviceConfig.SELinuxContext to
    # the policy's user-management domain (system_u:system_r:useradd_t:s0 on
    # Fedora's targeted policy); the binary must also be a valid entrypoint for
    # it.
    #
    # Such a domain typically also carries an unnamed catch-all transition --
    # `type_transition useradd_t etc_t:file shadow_t` -- whose named exceptions
    # only cover the final filenames. Since sysusers writes a temp file and
    # renames it into place, the temp name falls through to the catch-all and
    # /etc/group is created shadow_t, which system_dbusd_t cannot read: dbus
    # fails to start and the host does not boot.
    #
    # That is a labelling problem, not a domain problem, so it is fixed by
    # `systemd.sysusers.executable` -- a binary built with libselinux sets its
    # own create context and the transition never applies. See that option.
    systemd.services.system-manager-sysusers = lib.mkIf hasEntries {
      description = "Create System Users (system-manager)";
      wantedBy = [ "system-manager.target" ];
      before = [ "system-manager.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        Environment = [ "SYSUSERS_D=${sysusersDir}" ];
        ExecStart = cfg.executable;
      };
    };
  };
}
