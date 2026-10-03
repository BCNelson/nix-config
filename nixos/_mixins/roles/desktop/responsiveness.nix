{ config, lib, ... }:

# Keep the desktop interactive while the machine is saturated.
#
# cgroup v2 weights only matter under contention: an idle desktop still hands a
# build every core, so none of this costs throughput when nobody is waiting.
# The tiers, highest first:
#
#   user.slice      everything the logged-in user runs (compositor, apps)
#   system.slice    ordinary daemons
#   nix-daemon      builds -- whoever asked for them, a person or an agent
#
# The user-side split (session/app/background/lowprio) lives in
# home-manager/bcnelson/_mixins/lowprio.nix. IOWeight below only takes effect
# under BFQ or io.cost; hosts still on kyber/none ignore it (see the udev rule
# in nixos/sierra/default.nix).

{
  systemd.slices.user.sliceConfig = {
    # system.slice stays at the default 100, so the session wins 4:1 when both
    # want the CPU, rather than splitting it evenly.
    CPUWeight = 400;
    IOWeight = 400;
    # memory_recursiveprot is on, so this flows down to user@.service and is
    # what lets session.slice's own MemoryLow mean anything: a child's
    # protection is capped by its ancestors'.
    MemoryLow = "8G";
  };

  # SCHED_BATCH rather than "idle": the per-process idle policy can starve the
  # auto-update rebuild outright. The cgroup-level idle below is gentler -- it
  # only yields to siblings, and still gets system.slice's share when the
  # other daemons are quiet.
  nix.daemonCPUSchedPolicy = "batch";
  nix.daemonIOSchedClass = "best-effort";
  nix.daemonIOSchedPriority = 7;

  systemd.services.nix-daemon.serviceConfig = {
    CPUWeight = "idle";
    IOWeight = 10;
    # Throttle (reclaim from) a runaway build before the kernel starts
    # reclaiming from the compositor and the browser.
    MemoryHigh = "75%";
  };

  # docker.service is deliberately left alone: containers run in their own
  # system.slice/docker-*.scope, not under the daemon, so weighting the daemon
  # would do nothing for them.
  systemd.services.ollama.serviceConfig = lib.mkIf config.services.ollama.enable {
    CPUWeight = 50;
    IOWeight = 50;
  };

  boot.kernel.sysctl = {
    # The ratio defaults scale with RAM: 20% of 64G lets ~12G of dirty pages
    # pile up, and flushing that stalls anything that touches the disk. Cap it
    # in bytes so writeback starts early and stays smooth.
    "vm.dirty_bytes" = 536870912; # 512M
    "vm.dirty_background_bytes" = 134217728; # 128M
  } // lib.optionalAttrs config.zramSwap.enable {
    # Swapping to zram is a memcpy plus compress, not a seek, so the default
    # reluctance to swap is miscalibrated. Same reasoning as
    # ../thin-client/low-memory.nix.
    "vm.swappiness" = 180;
  };

  # MGLRU (on by default) protects pages used within the last min_ttl_ms from
  # eviction, so under pressure the kernel OOMs or kills instead of thrashing
  # the working set of whatever is on screen.
  systemd.tmpfiles.rules = [ "w- /sys/kernel/mm/lru_gen/min_ttl_ms - - - - 1000" ];
}
