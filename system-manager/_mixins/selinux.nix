{ pkgs, lib, config, ... }:
let
  selinuxModule = pkgs.callPackage ../../pkgs/nix-store-selinux.nix { };
  bootstrapSentinel = "/var/lib/system-manager/nix-store-selinux-bootstrapped";
  versionState = "/var/lib/system-manager/nix-store-selinux.sha256";
  unconfinedContext = "system_u:system_r:unconfined_t:s0";
  # useradd_t is the targeted policy's domain for useradd/usermod/userdel; it
  # has scoped rules to manage shadow_t, passwd_file_t, and group_t. Much
  # tighter than unconfined_t. Safe to use only for a binary that sets its own
  # SELinux create context -- see hostSysusers below.
  useraddContext = "system_u:system_r:useradd_t:s0";

  # systemd-sysusers must be built with libselinux. The stock nixpkgs systemd
  # is not (withSelinux defaults to false), so its systemd-sysusers cannot call
  # setfscreatecon() before creating the temp file it renames over /etc/group.
  # The label then falls to whatever type transition the domain carries, and
  # useradd_t's is an unnamed catch-all to shadow_t -- its named exceptions
  # match only the final filenames, never the temp name. An unreadable
  # /etc/group stops dbus-broker, which stops the display manager, and logind
  # needs the bus so there is no TTY either. That bricked this host on
  # 2026-07-31; recovery took a ZFSBootMenu chroot.
  #
  # With libselinux the binary sets its own create context, the transition never
  # applies, and the label is correct by construction -- no relabelling, and
  # useradd_t stays safe.
  #
  # Fedora's /usr/bin/systemd-sysusers is also built with libselinux and would
  # cost nothing to build, but it cannot be used: SELinuxContext= is a domain
  # transition, which requires `entrypoint` on the target binary, and useradd_t
  # holds entrypoint on nix_store_t, user_home_t and useradd_exec_t only -- not
  # bin_t. Granting entrypoint on all of bin_t would make every system binary a
  # legal way into the user-management domain. A /nix/store binary is already
  # covered by pkgs/nix-store-selinux.nix, so this costs one systemd build
  # instead.
  systemdSelinux = pkgs.systemd.override { withSelinux = true; };

  # SELinux support here is load-bearing and invisible: lose it and the next
  # activation mislabels /etc/group and the host stops booting. Assert it at
  # build time so that regression is a failed build rather than a failed boot.
  #
  # Ask the binary, do not inspect the ELF. systemd dlopens libselinux rather
  # than linking it, so ldd and `patchelf --print-needed` report nothing on a
  # correctly built systemd -- the only honest signal is its own feature list,
  # which prints -SELINUX for the stock build and +SELINUX for this one.
  sysusersExecutable =
    let
      checked = pkgs.runCommand "systemd-sysusers-selinux" { } ''
        bin=${systemdSelinux}/bin/systemd-sysusers
        if ! "$bin" --version | grep -qF '+SELINUX'; then
          echo "ERROR: $bin reports -SELINUX." >&2
          echo "Without it sysusers cannot set an SELinux create context; it" >&2
          echo "would mislabel /etc/group and leave the host unbootable." >&2
          exit 1
        fi
        mkdir -p "$out/bin"
        ln -s "$bin" "$out/bin/systemd-sysusers"
      '';
    in
    "${checked}/bin/systemd-sysusers";

  # Account files, plus the `-` backups that get rewritten by the same code path.
  accountFiles = [
    "/etc/passwd"
    "/etc/group"
    "/etc/shadow"
    "/etc/gshadow"
    "/etc/passwd-"
    "/etc/group-"
    "/etc/shadow-"
    "/etc/gshadow-"
  ];

  # Deliberately the host's restorecon, not a Nix one: it has to agree with the
  # running policy store. Generated units carry a /nix/store-only PATH, so the
  # search path is set explicitly rather than inherited. Silent no-op on hosts
  # without SELinux, and never fails — a relabel problem must not fail an
  # activation.
  relabelAccountFiles = pkgs.writeShellScript "relabel-account-files" ''
    export PATH=/usr/sbin:/usr/bin:/sbin:/bin''${PATH:+:$PATH}
    command -v selinuxenabled >/dev/null 2>&1 || exit 0
    selinuxenabled || exit 0
    command -v restorecon >/dev/null 2>&1 || exit 0
    for f in ${lib.escapeShellArgs accountFiles}; do
      [ -e "$f" ] || continue
      # -F is required: without it restorecon declines to reset a type it
      # considers customizable, and shadow_t on /etc/group is exactly that.
      restorecon -F -v "$f" || true
    done
    exit 0
  '';

  hasSysusers = config.systemd.sysusers.rules != [ ] || config.systemd.sysusers.settings != { };
  writesAccountFiles = config.services.userborn.enable || hasSysusers;
in
{
  options.environment.etc = lib.mkOption {
    type = lib.types.attrsOf (lib.types.submodule {
      options.selinuxRelabel = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether to emit a tmpfiles `Z` rule restoring this entry's SELinux
          label after system-manager activation. Set to false for entries
          that intentionally want to keep their /nix/store xattrs or are
          managed by another labelling mechanism.
        '';
      };
    });
  };

  config = {
    systemd.sysusers.executable = sysusersExecutable;

    system-manager.preActivationAssertions.selinuxRefresh = {
      enable = true;
      script = ''
        if ! command -v getenforce >/dev/null 2>&1 || [ "$(getenforce)" != "Enforcing" ]; then
          exit 0
        fi
        if [ ! -f ${bootstrapSentinel} ]; then
          echo "nix_store SELinux policy not bootstrapped on this host." >&2
          echo "Run 'just bootstrap-selinux' once to install the policy and label /nix/store." >&2
          exit 0
        fi
        pp=${selinuxModule}/nix_store.pp
        new_sha=$(sha256sum "$pp" | cut -d' ' -f1)
        if [ -f ${versionState} ] && [ "$(cat ${versionState})" = "$new_sha" ]; then
          exit 0
        fi
        echo "Refreshing nix_store SELinux module..."
        semodule -i "$pp" || exit 1
        echo "$new_sha" > ${versionState}
        exit 0
      '';
    };

    # system-manager preserves source xattrs when placing files in /etc, so
    # everything it touches inherits nix_store_t instead of the file_contexts
    # default (etc_t, systemd_unit_file_t, bin_t, ...). systemd-tmpfiles is
    # the only post-activation hook system-manager exposes — it runs against
    # /etc/tmpfiles.d/* immediately after etc files are placed — so we emit a
    # `Z` (recursive restorecon) for every managed entry. Idempotent: paths
    # already at the right label aren't rewritten.
    systemd.tmpfiles.rules = lib.pipe config.environment.etc [
      (lib.filterAttrs (_: e: e.enable && e.selinuxRelabel))
      (lib.mapAttrsToList (target: _: "Z /etc/${target} - - - - -"))
    ];

    # Force these services into unconfined_t at exec time. systemd's User=
    # switches the UID but the SELinux domain stays init_t by default, which
    # blocks reads/writes outside /nix/store. SELinuxContext= calls setexeccon
    # before exec so the process (and its children) start in the chosen domain.
    # Both of these write /etc/{passwd,group,shadow,gshadow}, and as init_t both
    # die on "Failed to open /etc/gshadow: Permission denied" -- init_t holds
    # only { getattr relabelfrom relabelto } on files -- so they need a domain
    # that can write shadow_t.
    #
    # They get different treatment because only one of them can be fixed
    # properly. sysusers runs a libselinux-linked binary (see systemdSelinux),
    # so it labels correctly on its own and useradd_t is safe. userborn has no
    # such option: it is not built with libselinux and exposes no flag to be,
    # so under useradd_t it would mislabel
    # /etc/group exactly as sysusers did. It gets the mitigation instead --
    # unconfined_t, which can write shadow_t, has no catch-all etc_t transition
    # (worst case degrades to etc_t, which dbus can read: wrong, but bootable),
    # and unlike useradd_t holds relabelto on passwd_file_t so the relabel
    # afterwards is actually permitted. Disabled on redo-3, armed for any host
    # that turns it on.
    systemd.services = lib.mkMerge [
      (lib.mkIf config.services.userborn.enable {
        userborn.serviceConfig = {
          SELinuxContext = unconfinedContext;
          ExecStartPost = "${relabelAccountFiles}";
        };
      })
      # The condition mirrors the one guarding the service in the sysusers
      # module -- reading systemd.services here instead would be a cycle -- so
      # the two have to stay in step.
      (lib.mkIf hasSysusers {
        system-manager-sysusers.serviceConfig.SELinuxContext = useraddContext;
      })
      # Safety net, deliberately kept after the causes above were fixed. A bad
      # label on /etc/group costs a ZFSBootMenu rescue with no TTY, which is
      # expensive enough not to bet on having enumerated every writer of these
      # files. Ordered like systemd-tmpfiles-setup
      # (DefaultDependencies=no, after local-fs so /nix is mounted, before
      # sysinit) which puts it ahead of dbus.socket at ~2.8s on this host, so a
      # mislabel from any source self-heals instead of needing ZFSBootMenu.
      (lib.mkIf writesAccountFiles {
        selinux-account-file-labels = {
          description = "Restore SELinux labels on account files";
          wantedBy = [ "sysinit.target" ];
          before = [ "sysinit.target" "shutdown.target" ];
          after = [ "local-fs.target" ];
          conflicts = [ "shutdown.target" ];
          unitConfig = {
            DefaultDependencies = false;
            ConditionSecurity = "selinux";
          };
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            SELinuxContext = unconfinedContext;
            ExecStart = "${relabelAccountFiles}";
          };
        };
      })
      (lib.mapAttrs'
        (name: _: lib.nameValuePair "home-manager-${name}" {
          serviceConfig.SELinuxContext = unconfinedContext;
        })
        config.home-manager.users)
    ];
  };
}
