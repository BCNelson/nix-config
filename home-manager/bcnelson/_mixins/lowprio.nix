{ config, lib, pkgs, ... }:
# The user-side priority tiers, under user@.service. The system-side half
# (user.slice vs system.slice vs nix-daemon) is
# nixos/_mixins/roles/desktop/responsiveness.nix.
#
#   session.slice     compositor, audio, portals        CPUWeight 1000
#   app.slice         apps, terminals, the herdr server CPUWeight 100 (default)
#   background.slice  baloo, syncthing                  CPUWeight 30 (default)
#   lowprio.slice     agent commands, `lowprio <cmd>`   CPUWeight 20
#
# Weights only bite under contention, so an otherwise idle machine still gives
# a lowprio build every core.
{
  home.packages = [ pkgs.lowprio ];

  # Every agent command lands here, one transient scope each (see
  # pkgs/lowprio). The slice is the shared budget: ten parallel builds split
  # weight 20 between them rather than getting 20 each.
  systemd.user.slices.lowprio = {
    Unit.Description = "Deprioritized batch work (agent commands, lowprio)";
    Slice = {
      CPUWeight = 20;
      IOWeight = 10;
      # Throttle builds by reclaiming their own memory first, before the
      # kernel starts taking it from the session.
      MemoryHigh = "70%";
      # systemd-oomd watches the slice and, past the default 60% pressure for
      # 30s, kills the child scope reclaiming hardest: one command, never the
      # agent or herdr server that launched it.
      ManagedOOMMemoryPressure = "kill";
    };
  };

  # Upstream unit, so a drop-in rather than a definition that would shadow it.
  # MemoryLow is carved out of user.slice's protection (set in
  # responsiveness.nix); without that it would be capped at zero.
  xdg.configFile."systemd/user/session.slice.d/responsiveness.conf".text = ''
    [Slice]
    CPUWeight=1000
    IOWeight=1000
    MemoryLow=2G
  '';

  # Syncthing is background work, but the module leaves it in app.slice where
  # it competes with the browser on equal terms.
  systemd.user.services.syncthing = lib.mkIf config.services.syncthing.enable {
    Service.Slice = "background.slice";
  };
}
