# agenix + agenix-rekey for system-manager hosts.
#
# The NixOS side of this repo gets secrets from `inputs.agenix.nixosModules.default`
# (see ../../nixos/secrets.nix). That module cannot be reused here.
#
# system-manager has no `system.activationScripts` hook: its engine's activate()
# runs exactly four steps -- etc files, tmpfiles, users, services -- with no
# generic script mechanism. What little of that option tree exists upstream is
# stub-only, declared purely to absorb writes and discard them
# (nix/modules/upstream/sops-nix.nix, and `system.activationScripts.users` in
# nix/modules/upstream/nixpkgs/default.nix). The stubs make a module *evaluate*;
# they never make it *work*.
#
# The upstream position on what to do instead is explicit. From
# numtide/system-manager#81, maintainer r-vdp: "A better solution would be for
# sops to decrypt secrets through a systemd service instead of an activation
# script, which is the general direction in which things are going anyway." And
# from docs/site/how-to/import-nixos-module.md: "The actual secret decryption is
# handled differently in system-manager through a systemd service."
#
# sops-nix could follow that advice because it made systemd activation a
# first-class mode upstream (`sops.useSystemdActivation`), which is why
# system-manager can test it end to end. agenix has no equivalent: it emits a
# systemd unit only when `sysusersEnabled` is true, and that check reads
# `config.systemd.sysusers.enable`, an option system-manager does not have
# (../../contrib/system-manager-sysusers/module.nix is settings-driven and
# declares no `enable`), so the expression throws before the branch is reachable.
# Its unit also chowns the mountpoint to a `keys` group that does not exist on a
# foreign distro. Neither ryantm/agenix nor oddlama/agenix-rekey has any
# system-manager support or open issue tracking it, so there is nothing upstream
# to wait for.
#
# So the option surface agenix declares is reproduced here and the installer is
# re-expressed as the systemd service upstream asks for, with the same unit
# wiring system-manager's own sops-nix test uses. `age.rekey.*`,
# `age.secrets.*.rekeyFile` and the generator machinery all come from the
# upstream agenix-rekey module, which is portable as-is: it only ever touches
# `assertions`, `warnings` and its own `age.*` options, all of which
# system-manager provides.
{ config, lib, pkgs, inputs, hostname, ... }:
let
  inherit (lib) mkOption types literalExpression;

  cfg = config.age;
  GetHostsData = import ../../hosts;

  # Secrets are decrypted with the host's SSH host key, which is exactly the
  # pubkey agenix-rekey re-encrypts to below, so a host can only ever read its
  # own rekeyed copies.
  installSecret = secret: ''
    echo "[agenix] decrypting '${secret.file}' -> '${secret.path}'"
    mkdir -p "$(dirname "${secret.path}")"
    # Clear a scratch file left by an aborted earlier run. It is created 0400
    # by the umask below, and the unit's CapabilityBoundingSet deliberately
    # excludes CAP_DAC_OVERRIDE -- the capability that normally lets root
    # ignore file modes -- so a leftover is not merely stale, it is impossible
    # to reopen for writing and every subsequent run fails with
    # "permission denied" until it is removed by hand. Unlinking needs write on
    # the directory, not the file, so this works without that capability.
    rm -f "${secret.path}.tmp"
    (
      umask u=r,g=,o=
      ${cfg.ageBin} --decrypt "''${IDENTITIES[@]}" -o "${secret.path}.tmp" "${secret.file}"
    )
    chmod ${secret.mode} "${secret.path}.tmp"
    chown ${secret.owner}:${secret.group} "${secret.path}.tmp"
    # Rename last: a reader either sees the previous content or the new one,
    # never a partially written file. Per-file atomicity is what actually
    # matters here, which is why there is no generation directory the way
    # upstream agenix has one.
    mv -f "${secret.path}.tmp" "${secret.path}"
  '';

  installScript = pkgs.writeShellScript "agenix-install-secrets" ''
    set -euo pipefail

    IDENTITIES=()
    for identity in ${toString cfg.identityPaths}; do
      test -r "$identity" || continue
      test -s "$identity" || continue
      IDENTITIES+=(-i "$identity")
    done
    if [ "''${#IDENTITIES[@]}" -eq 0 ]; then
      echo "[agenix] no readable identity in ${toString cfg.identityPaths}" >&2
      exit 1
    fi

    ${lib.concatStringsSep "\n" (map installSecret (builtins.attrValues cfg.secrets))}

    # Retire secrets dropped from the config. The tmpfs outlives the service
    # (it is a mount unit now, not a RuntimeDirectory that systemd clears on
    # stop), so a secret deleted from the config would otherwise sit decrypted
    # in ${cfg.secretsDir} until the next reboot. Done after writing rather than
    # before, so there is no window where a live secret is missing.
    #
    # Only prunes ${cfg.secretsDir} itself -- a secret pointed at a path
    # elsewhere is left for whoever owns that location.
    shopt -s nullglob dotglob
    for existing in ${cfg.secretsDir}/*; do
      case "''${existing##*/}" in
        ${lib.concatMapStringsSep "|" (secret: secret.name) (builtins.attrValues cfg.secrets)}) ;;
        *)
          echo "[agenix] removing retired secret '$existing'"
          rm -rf -- "$existing"
          ;;
      esac
    done

    exit 0
  '';

in
{
  imports = [ inputs.agenix-rekey.nixosModules.default ];

  options = {
    # agenix-rekey names a node through nix/target-name.nix, which reads
    # `config.networking.hostName` and otherwise falls through to
    # `config.home.username` -- an option that does not exist at the top level of
    # a system-manager config. system-manager declares only
    # `networking.enableIPv6`, so the name has to be declared here.
    networking.hostName = mkOption {
      type = types.str;
      description = "This host's name, as agenix-rekey identifies it.";
    };

    age = {
      ageBin = mkOption {
        type = types.str;
        default = "${pkgs.age}/bin/age";
        defaultText = literalExpression ''"''${pkgs.age}/bin/age"'';
        description = "The age binary used to decrypt secrets at activation.";
      };

      secretsDir = mkOption {
        type = types.path;
        default = "/run/agenix";
        description = ''
          Where decrypted secrets are exposed. Backed by a dedicated tmpfs
          mounted by systemd -- see {option}`age.secretsMountOptions`.
        '';
      };

      secretsMountOptions = mkOption {
        type = types.str;
        default = "nodev,nosuid,mode=0751,noswap";
        description = ''
          Mount options for the tmpfs backing {option}`age.secretsDir`.

          `noswap` is the reason this is a dedicated filesystem rather than a
          plain directory on /run: it is what upstream agenix gets from mounting
          ramfs, i.e. plaintext can never be paged out to disk. Kept even on
          hosts whose swap is zram (where a swapped page stays in RAM anyway),
          so the guarantee does not silently depend on how a given host
          configures swap. Requires Linux >= 6.4; drop `noswap` for older
          kernels.

          `mode=0751` is owner-only write with world traverse, so an
          unprivileged consumer can open its own 0400 secret by name without
          being able to list the directory.
        '';
      };

      identityPaths = mkOption {
        type = types.listOf types.str;
        default = [ "/etc/ssh/ssh_host_ed25519_key" ];
        description = ''
          Private keys tried when decrypting. Unreadable or empty entries are
          skipped at runtime, so listing a key a given host lacks is harmless.

          Deliberately ed25519 only: every host in ../../hosts/data is
          registered with an ed25519 pubkey, so the RSA host key could never
          decrypt anything here and reading it was pure extra exposure.
        '';
      };

      secrets = mkOption {
        default = { };
        description = "Secrets decrypted into {option}`age.secretsDir` at activation.";
        type = types.attrsOf (types.submodule (submod: {
          options = {
            name = mkOption {
              type = types.str;
              default = submod.config._module.args.name;
              defaultText = literalExpression "config._module.args.name";
              description = "Filename this secret takes inside {option}`age.secretsDir`.";
            };
            file = mkOption {
              type = types.path;
              description = "The age file to decrypt. Set for you when `rekeyFile` is used.";
            };
            path = mkOption {
              type = types.str;
              default = "${cfg.secretsDir}/${submod.config.name}";
              defaultText = literalExpression ''"''${cfg.secretsDir}/''${config.name}"'';
              description = "Where the decrypted secret is readable.";
            };
            mode = mkOption {
              type = types.str;
              default = "0400";
              description = "Permissions of the decrypted file, in octal.";
            };
            owner = mkOption {
              type = types.str;
              default = "0";
              description = "User who owns the decrypted file.";
            };
            group = mkOption {
              type = types.str;
              # Follow the owner's primary group, as agenix does. Defaulting to
              # root instead would hand `owner = "someuser"` a root-group file,
              # which happens to work for a 0400 secret the owner reads but
              # silently breaks the moment a group-readable mode is wanted.
              default = config.users.users.${submod.config.owner}.group or "0";
              defaultText = literalExpression ''config.users.users.''${config.owner}.group or "0"'';
              description = "Group who owns the decrypted file.";
            };
          };
        }));
      };
    };
  };

  config = {
    networking.hostName = hostname;

    # Same masterIdentities and storage layout as ../../nixos/secrets.nix, so
    # `just rekey` treats a system-manager host no differently from a NixOS one
    # and a secret shared with a NixOS host decrypts to the identical value.
    age.rekey = {
      storageMode = "local";
      # Bootstrapping a brand new host: this directory has to be tracked by git
      # *before* the first `just rekey`, because agenix-rekey resolves secrets
      # through `builtins.path { path = localStorageDir; }`. Until it is, eval
      # fails with "Path '...' does not exist in Git repository" instead of the
      # friendly "please run agenix rekey" assertion. Create it with a throwaway
      # tracked file (`.gitkeep`) and `git add` it; the first rekey prunes that
      # file and leaves the real .age behind, which keeps the directory tracked
      # from then on.
      localStorageDir = ../../secrets/hosts/${hostname};
      hostPubkey = (GetHostsData hostname).hostKey;
      masterIdentities = [
        {
          identity = ../../secrets/masterKeys/yubikey5cblack.pub;
          pubkey = "age1yubikey1qgw5sthxazuy96nq4cnldd7wydn4jf59cc5sc5fglmjnh2getqu4g2rmyfj";
        }
        {
          identity = ../../secrets/masterKeys/soloback.hmac;
          pubkey = "age1qknx4qlm8qse85afs5np42kf2rsh28j9jvyzdd3n7gljpclhep9qrt2qrt";
        }
      ];
      agePlugins = [ pkgs.age-plugin-fido2-hmac ];
    };

    # Unit wiring copied from sops-nix's sops-install-secrets, which is the only
    # secrets provisioner system-manager actually tests (testFlake/vm-tests.nix,
    # added in numtide/system-manager#270). Both halves are load-bearing:
    #
    #   sysinit.target             pulls the unit in at boot. ${cfg.secretsDir} is
    #                              a tmpfs, so it is empty after every reboot and
    #                              nothing else would repopulate it until the next
    #                              switch.
    #   sysinit-reactivation.target is restarted by system-manager-engine on every
    #                              activation, deliberately before the ordinary
    #                              services, so secrets are in place before
    #                              anything that consumes them starts.
    # Declared rather than mounted by the service itself. A service that
    # creates its own mount needs CAP_SYS_ADMIN and cannot use any namespacing
    # sandbox option, because those imply PrivateMounts=, whose documented
    # effect is that "any file system mount points established ... by the unit's
    # processes will be private to them and not be visible to the host" -- the
    # secrets would land somewhere nothing else can see. Letting systemd own the
    # mount keeps `noswap` and still allows the sandbox below.
    systemd.mounts = lib.mkIf (cfg.secrets != { }) [{
      what = "tmpfs";
      where = cfg.secretsDir;
      type = "tmpfs";
      options = cfg.secretsMountOptions;
      unitConfig.DefaultDependencies = "no";
      before = [ "sysinit.target" ];
      wantedBy = [ "sysinit.target" ];
    }];

    systemd.services.agenix-install-secrets = lib.mkIf (cfg.secrets != { }) {
      description = "Decrypt agenix secrets";
      wantedBy = [ "sysinit.target" ];
      before = [ "sysinit.target" "sysinit-reactivation.target" ];
      requiredBy = [ "sysinit-reactivation.target" ];
      # DefaultDependencies=no is what lets this run before sysinit.target at
      # all, which costs the implicit ordering against local-fs.target -- hence
      # naming it explicitly. userborn/sysusers matter because secrets are
      # chowned to accounts those units create.
      after = [ "local-fs.target" "systemd-sysusers.service" "userborn.service" ];
      unitConfig = {
        DefaultDependencies = "no";
        # Pulls in and orders after the tmpfs unit below, so the service never
        # writes secrets into the underlying directory before the filesystem
        # that is supposed to hold them is mounted over it.
        RequiresMountsFor = cfg.identityPaths ++ [ cfg.secretsDir ];
      };
      path = [ pkgs.coreutils ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = installScript;

        # Everything below is defence in depth behind the SELinux confinement in
        # ./selinux.nix. Sized to what the script actually does: read one key,
        # write files, chown them to their owner.
        #
        # CAP_CHOWN for the chown, CAP_FOWNER for the chmod on a file that is
        # about to change owner, CAP_DAC_READ_SEARCH to open the 0600 root host
        # key. Notably absent: CAP_SYS_ADMIN, which the old self-mount needed.
        CapabilityBoundingSet = [ "CAP_CHOWN" "CAP_FOWNER" "CAP_DAC_READ_SEARCH" ];
        AmbientCapabilities = [ ];

        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        # Decryption is entirely local -- age never dials out. An exfiltration
        # path is simply removed rather than restricted.
        PrivateNetwork = true;
        IPAddressDeny = "any";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        ProtectProc = "invisible";
        ProcSubset = "pid";
        ProtectControlGroups = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" "~@resources" "@chown" ];

        # ProtectSystem=strict makes the whole tree read-only, so every
        # directory a secret lands in has to be opened back up -- the tmpfs
        # itself, plus any secret configured to a path outside it. "-" tolerates
        # a path that does not exist yet.
        ReadWritePaths = lib.unique (
          [ "-${cfg.secretsDir}" ]
          ++ map (secret: "-" + builtins.dirOf secret.path) (builtins.attrValues cfg.secrets)
        );
      };
    };
  };
}
