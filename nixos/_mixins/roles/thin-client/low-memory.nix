{ lib, ... }:

# Fit in 2 GB of RAM.
#
# Idle is not the problem: measured on a Wyse 3040-equivalent VM, the base
# system sits at 266 MB used with 1.7 GB available and 11 running services.
# The problem is the one moment this machine does something demanding -- it
# pulls a whole system closure over the network and unpacks it -- and the
# defaults are tuned for a workstation, not for an Atom with 2 GB.

{
  # Compressed swap in RAM. zstd gets roughly 3:1 on anonymous pages, so a
  # device sized at 100% of RAM costs only what is actually stored in it while
  # buying multiples of that in headroom. This is what stands between a memory
  # spike and the OOM killer, since there is no fast disk to swap to -- the
  # eMMC is both slow and wearing out.
  zramSwap = {
    enable = lib.mkDefault true;
    memoryPercent = lib.mkDefault 100;
  };

  boot.kernel.sysctl = {
    # Swapping to zram is RAM-to-RAM, so the usual reluctance to swap is
    # miscalibrated here: paging out a cold page costs a memcpy and a compress,
    # not a seek. The kernel maximum is 200; 180 is the low-RAM-device profile.
    "vm.swappiness" = lib.mkDefault 180;

    # Swap readahead exists to amortise disk seeks. zram has none, so faulting
    # in 8 pages to use 1 just burns CPU decompressing the other 7 -- and CPU is
    # the scarce resource on an Atom x5.
    "vm.page-cluster" = lib.mkDefault 0;

    # Start reclaiming earlier and more gently. The default watermarks are set
    # for machines with room to be surprised; here the gap between "fine" and
    # "OOM killer" is a few hundred megabytes, so it pays to begin reclaim well
    # before the wall and to skip the boost that makes reclaim come in bursts.
    "vm.watermark_scale_factor" = lib.mkDefault 125;
    "vm.watermark_boost_factor" = lib.mkDefault 0;
  };

  # Peak memory during an update is set by how many NARs nix decompresses at
  # once. The default 16 substitution jobs over 25 connections is sized for a
  # workstation on fast internet; here it is 16 simultaneous zstd streams
  # competing for the same 2 GB. Four keeps a gigabit LAN busy on this CPU.
  nix.settings = {
    max-substitution-jobs = lib.mkDefault 4;
    http-connections = lib.mkDefault 8;
  };

  # systemd's default is to let the journal grow to 10% of the filesystem. On a
  # 8 GB eMMC that is both a lot of writes on flash with limited cycles and a
  # slow read for anything that tails it.
  services.journald.extraConfig = lib.mkDefault ''
    SystemMaxUse=64M
    RuntimeMaxUse=16M
  '';

  # If something does run away, kill it deliberately rather than letting the
  # machine thrash until the watchdog gives up. Enabled by default upstream;
  # stated here because on this hardware it is load-bearing, not a nicety.
  systemd.oomd.enable = lib.mkDefault true;

  # A tmpfs /tmp is RAM. Whatever writes there is competing with the closure
  # being unpacked; put it on disk, slow as that disk is.
  boot.tmp.useTmpfs = lib.mkDefault false;
}
