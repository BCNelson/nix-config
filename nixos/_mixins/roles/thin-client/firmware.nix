{ config, lib, pkgs, ... }:

# Ship only the firmware this hardware actually asks for.
#
# linux-firmware is 770 MB and 39% of a thin client's entire closure -- by far
# the largest single item, bigger than the kernel, systemd and every package
# combined. Almost all of it is for hardware a Wyse 3040 does not have: GPUs from
# three vendors, dozens of wifi chipsets, cellular modems, sound DSPs.
#
# The curated set below is ~3 MB. That is not a micro-optimisation: it is the
# difference between two generations fitting on an 8 GB eMMC and not, and between
# a 2 GB and a 1.2 GB download on every config change.
#
# Getting this wrong is unpleasant to debug on a headless appliance, so:
#   - a pattern that matches nothing fails the BUILD, not the boot, which turns
#     an upstream firmware rename into a red CI job instead of a dead NIC;
#   - `full = true` restores the whole set for bring-up or unknown hardware;
#   - to find out what a real machine wants, boot it once with `full = true` and
#     read `dmesg | grep -i firmware` -- the kernel names every file it requests,
#     including the ones it failed to find.

let
  cfg = config.services.bcnelson.thinClient.firmware;

  # Copy matching files out of the uncompressed tree; NixOS compresses whatever
  # lands in hardware.firmware on the way in, so plain files are correct here.
  subset = pkgs.runCommandLocal "linux-firmware-thin-client" { } ''
    shopt -s nullglob
    cd ${pkgs.linux-firmware}/lib/firmware
    mkdir -p "$out/lib/firmware"
    total=0
    for pattern in ${lib.escapeShellArgs cfg.patterns}; do
      matched=0
      for f in $pattern; do
        [ -f "$f" ] || continue
        install -Dm444 "$(readlink -f "$f")" "$out/lib/firmware/$f"
        matched=$((matched + 1))
      done
      if [ "$matched" -eq 0 ]; then
        echo "error: firmware pattern '$pattern' matched nothing in linux-firmware." >&2
        echo "It was probably renamed upstream. Fix the pattern in" >&2
        echo "nixos/_mixins/roles/thin-client/firmware.nix, or drop it if the" >&2
        echo "hardware no longer needs it." >&2
        exit 1
      fi
      echo "  $pattern -> $matched file(s)"
      total=$((total + matched))
    done
    echo "included $total firmware files"
  '';
in
{
  options.services.bcnelson.thinClient.firmware = {
    full = lib.mkEnableOption ''
      shipping the complete linux-firmware set instead of the curated subset.
      Useful for bringing up hardware whose firmware needs are not yet known --
      boot with this on, read `dmesg | grep -i firmware`, then encode what it
      asked for in `patterns` and turn it back off
    '';

    patterns = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      description = ''
        Glob patterns, relative to lib/firmware, of the files to include. Each
        one must match at least something or the build fails.
      '';
      default = [
        # Realtek gigabit ethernet (RTL8111/8168 on the 3040). The whole family
        # is 0.2 MB, so there is nothing to gain by narrowing it to one variant
        # and something to lose if a unit turns out to have a different one.
        "rtl_nic/*"

        # Intel Wireless-AC 3165, the optional wifi module, runs 7265D ucode.
        # 2 MB for the family. Harmless on the wired-only units.
        "iwlwifi-7265*"

        # Bluetooth on that same combo card. Narrowed deliberately: all of
        # intel/ibt-* is 19 MB, this generation is 0.1 MB.
        "intel/ibt-hw-37.8*"

        # Cherry Trail's audio DSP (legacy SST path). Drop this if the units
        # never need sound.
        "intel/fw_sst_22a8.bin"

        # Deliberately absent: i915. Cherryview predates the display
        # microcontroller, and `i915/*chv*` matches nothing in linux-firmware --
        # the 26 MB in that directory is all for later generations.
      ];
    };
  };

  config = {
    # Normal priority, not mkForce: the generated hardware-configuration.nix
    # sets this with mkDefault, which this beats, while still letting an
    # individual host override with mkForce if it turns out to need everything.
    hardware.enableRedistributableFirmware = false;
    hardware.firmware = [ (if cfg.full then pkgs.linux-firmware else subset) ];

    # nixos-generate-config writes
    #   hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
    # so turning the firmware set off would silently disable microcode updates
    # too. On an Atom that means shipping without Spectre/Meltdown mitigations,
    # which is not a trade anyone would make on purpose. Microcode does not come
    # from linux-firmware, so keeping it costs nothing here.
    hardware.cpu.intel.updateMicrocode = true;
  };
}
