{ lib, ... }:

# Trim the base system down to what an appliance that only ever *runs* a
# prebuilt closure needs.
#
# This is not premature tidying. Every byte here is fetched over the network on
# every update and then stored twice -- old generation and new -- on an eMMC
# measured in tens of gigabytes. The measured starting point was 2.3 GiB for a
# host with no users at all, of which roughly a third was firmware and another
# 400 MB was tooling the machine is structurally incapable of using.
#
# The cuts below follow from one fact: this host never builds, never evaluates,
# and never rebuilds. Anything that exists to support those activities is dead
# weight, so removing it is not a trade-off, it is deleting something that could
# not have worked anyway.

{
  # nixos-rebuild and friends drag in a full python3 (~135 MB) to run a rebuild
  # this machine has nowhere near the memory to attempt. switch-to-configuration
  # lives in the system closure itself, not here, so activation is unaffected.
  system.disableInstallerTools = true;

  # /etc/nix/registry.json pins the flake inputs by store path, which anchors an
  # entire copy of the nixpkgs source (~200 MB) into the closure. Useful on a
  # machine where you type `nix run nixpkgs#...`; pure cost on one that cannot
  # evaluate nixpkgs at all.
  nix.registry = lib.mkForce { };
  nix.nixPath = lib.mkForce [ ];

  # Manpages, info pages and the NixOS manual, on a headless appliance nobody
  # reads documentation on. Also drops texinfo and gettext from the closure.
  documentation = {
    enable = lib.mkDefault false;
    nixos.enable = lib.mkDefault false;
  };

  # perl, rsync and strace, installed by default for interactive convenience.
  environment.defaultPackages = lib.mkForce [ ];

  # NetworkManager turns ModemManager on by default for WWAN dongles, which
  # costs ~67 MB in libqmi alone. These have an ethernet port.
  networking.modemmanager.enable = lib.mkForce false;

  # No desktop, so nothing is reading a dconf database.
  programs.dconf.enable = lib.mkForce false;

  # An appliance that reinstalls itself from the cache is not the place to be
  # flashing firmware, and this keeps another network-facing daemon off the box.
  # Dell BIOS updates are better done deliberately, from the vendor tooling.
  services.fwupd.enable = lib.mkForce false;

  # A small ESP with several kernels in it fills up and then the *next* update
  # fails at the bootloader step, which is a miserable way to find out.
  boot.loader.systemd-boot.configurationLimit = lib.mkDefault 5;

  # Old generations are the other half of the disk pressure. common.nix keeps a
  # week; on this hardware that is several closures' worth of eMMC. Only the
  # retention is overridden -- the daily schedule from common.nix still applies.
  nix.gc.options = "--delete-older-than 3d";
}
