{ pkgs, lib, config, ... }:
let
  selinuxModule = pkgs.callPackage ../../pkgs/nix-store-selinux.nix { };
  bootstrapSentinel = "/var/lib/system-manager/nix-store-selinux-bootstrapped";
  versionState = "/var/lib/system-manager/nix-store-selinux.sha256";
  unconfinedContext = "system_u:system_r:unconfined_t:s0";
  # useradd_t is the targeted policy's domain for useradd/usermod/userdel; it
  # has scoped rules to manage shadow_t, passwd_file_t, and group_t. Much
  # tighter than unconfined_t and exactly what userborn needs.
  useraddContext = "system_u:system_r:useradd_t:s0";
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
    systemd.services = lib.mkMerge [
      (lib.mkIf config.services.userborn.enable {
        userborn.serviceConfig.SELinuxContext = useraddContext;
      })
      (lib.mapAttrs'
        (name: _: lib.nameValuePair "home-manager-${name}" {
          serviceConfig.SELinuxContext = unconfinedContext;
        })
        config.home-manager.users)
    ];
  };
}
