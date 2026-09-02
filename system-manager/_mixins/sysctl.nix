{ config, lib, pkgs, ... }:
let
  cfg = config.system.sysctl;

  # null means "declared somewhere but deliberately unset", same as NixOS, so a
  # host can cancel an inherited key without the mixin emitting a line for it.
  settings = lib.filterAttrs (_: value: value != null) cfg;

  renderValue = value:
    if value == true then "1"
    else if value == false then "0"
    else toString value;

  sysctlConf = pkgs.writeText "system-manager-sysctl.conf" (
    lib.concatMapStrings
      (key: "${key} = ${renderValue (settings.${key})}\n")
      (lib.attrNames settings)
  );
in
{
  # system-manager counterpart of NixOS's `boot.kernel.sysctl`. It ships no
  # sysctl module of its own -- environment.etc and systemd are all this needs.
  #
  # Not named `boot.kernel.sysctl` like everything else here is named after its
  # NixOS twin: system-manager declares `boot` as a bare `types.raw` stub
  # (nix/modules/upstream/nixpkgs/default.nix) to swallow the NixOS-module uses
  # that don't apply to it, and a raw value cannot be a parent of nested
  # options -- declaring under it is an eval error, not a merge.
  options.system.sysctl = lib.mkOption {
    type = with lib.types; attrsOf (nullOr (oneOf [ bool int str ]));
    default = { };
    example = {
      "net.ipv4.ip_unprivileged_port_start" = 80;
    };
    description = lib.mdDoc ''
      Kernel parameters to set via {manpage}`sysctl(8)`, written to
      {file}`/etc/sysctl.d/60-system-manager.conf` and applied on activation.
      Set a key to `null` to leave it at the kernel default.
    '';
  };

  config = lib.mkIf (settings != { }) {
    # The file is what makes the settings survive a reboot: the distro's
    # systemd-sysctl.service reads /etc/sysctl.d early at boot, long before
    # anything system-manager owns runs. Copied at 0644 rather than symlinked so
    # it gets an etc_t relabel from ./selinux.nix instead of inheriting
    # nix_store_t, which systemd-sysctl's domain has no reason to be able to
    # read.
    environment.etc."sysctl.d/60-system-manager.conf" = {
      source = sysctlConf;
      mode = "0644";
      replaceExisting = true;
    };

    # ...and the unit is what applies them *now*, on activation. Nothing in
    # system-manager restarts the distro's systemd-sysctl.service, so without
    # this a changed value would sit in /etc doing nothing until the next boot.
    #
    # ExecStart names the config by store path on purpose. system-manager only
    # reload-or-restarts a unit whose store path changed
    # (get_services_to_reload in system-manager-engine), so referencing the
    # content-addressed file is what ties "a value changed" to "the unit
    # re-runs". Pointing at /etc/sysctl.d/... instead would leave the unit
    # byte-identical across a value change and it would never fire.
    #
    # sysinit-reactivation.target is restarted before the ordinary services on
    # every activation; multi-user.target (rewritten to system-manager.target by
    # the systemd module) is what starts it the first time and at boot.
    systemd.services.system-manager-sysctl = {
      description = "Apply kernel sysctl settings";
      wantedBy = [ "multi-user.target" "sysinit-reactivation.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # --load fails loudly on an unknown key rather than skipping it, which
        # is the behaviour we want: a typo should fail the activation.
        ExecStart = "${pkgs.procps}/bin/sysctl --load=${sysctlConf}";
      };
    };
  };
}
