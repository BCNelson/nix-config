{ config, lib, pkgs, ... }:
let
  cfg = config.hardware.keyboard.qmk;
in
{
  # system-manager counterpart of nixos/_mixins/hardware/qmk.nix, named to match
  # `hardware.keyboard.qmk.enable` so a host reads the same either way, and
  # using the same pkgs.qmk-udev-rules so both stay on upstream QMK's list.
  #
  # The one wrinkle: upstream's raw-HID line is
  #   KERNEL=="hidraw*", MODE="0660", GROUP="plugdev", TAG+="uaccess", ...
  # and udev discards a whole rule line whose GROUP it cannot resolve. NixOS
  # ships a plugdev group so it works there; Fedora does not, and `udevadm test`
  # against the Keychron confirmed the fallout -- with the group missing the
  # device ends up TAGS=:seat: (no uaccess, VIA cannot open it), and with it
  # resolvable TAGS=:uaccess:udev-acl:seat:. So create the group.
  #
  # The group is intentionally left empty: access comes from the uaccess ACL
  # granted to whoever owns the active seat, not from membership. It exists so
  # udev keeps the line.
  options.hardware.keyboard.qmk.enable = lib.mkEnableOption ''
    udev rules for QMK keyboards, granting the active seat's user raw-HID
    access so VIA (desktop app or usevia.app over WebHID) can read and write
    the keymap, plus bootloader/DFU access for flashing firmware
  '';

  config = lib.mkIf cfg.enable {
    systemd.sysusers.rules = [ "g plugdev -" ];

    # Upstream ships this as 50-, i.e. below systemd's 73-seat-late.rules, which
    # is what actually runs the uaccess builtin -- keep the name so the ordering
    # is preserved. Copied at 0644 rather than symlinked: the store file is
    # 0555 and udev warns on executable rule files, and a copy also gets an
    # etc_t relabel from the selinux mixin instead of inheriting nix_store_t.
    environment.etc."udev/rules.d/50-qmk.rules" = {
      source = "${pkgs.qmk-udev-rules}/lib/udev/rules.d/50-qmk.rules";
      mode = "0644";
      replaceExisting = true;
    };
  };
}
