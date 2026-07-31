{ config, lib, ... }:
let
  cfg = config.hardware.uinput;
in
{
  # system-manager counterpart of NixOS's `hardware.uinput.enable`, named to
  # match so a host reads the same either way. Same three moving parts upstream
  # has: load the module, own a group, and hand that group the device node.
  #
  # Fedora ships neither the module-load nor the rule -- /dev/uinput is
  # root:root 0600 when something else happens to have loaded uinput, and
  # absent otherwise -- so dotool (voxtype's typing driver, see
  # home-manager/bcnelson/_mixins/programs/voxtype.nix) cannot open it.
  #
  # uaccess would be the lighter option, but it does not reach here: the
  # ACL is applied by systemd's 73-seat-late.rules, which only fires for
  # devices 71-seat.rules assigned to a seat, and that rule's subsystem list
  # has no `misc`. So this goes the group route, like NixOS.
  options.hardware.uinput = {
    enable = lib.mkEnableOption ''
      the uinput kernel module and a uinput group with access to
      {file}`/dev/uinput`, for userspace drivers that synthesise input events
    '';

    users = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "alice" ];
      description = ''
        Users to add to the uinput group. Membership is only picked up by new
        logins, so an existing session has to be restarted once after this
        first applies.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."modules-load.d/uinput.conf" = {
      mode = "0644";
      text = "uinput\n";
    };

    systemd.sysusers.rules = [ "g uinput -" ]
      ++ map (user: "m ${user} uinput") cfg.users;

    # OPTIONS+="static_node" is what makes the permissions apply to the node
    # udev creates from the module's devnode= hint at load time, rather than
    # only to a hotplug event that never comes for a misc device.
    #
    # Copied at 0644 rather than symlinked so it gets an etc_t relabel from the
    # selinux mixin instead of inheriting nix_store_t.
    environment.etc."udev/rules.d/60-uinput.rules" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="uinput", MODE="0660", OPTIONS+="static_node=uinput"
      '';
    };
  };
}
