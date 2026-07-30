{ ... }:

# Tier 1: every user, every machine.
#
# Anything added here lands on a 2 GB Wyse 3040 with an 8 GB eMMC just as much
# as on a workstation, so the bar is "a login is broken without it" rather than
# "this is nice to have". Convenience belongs in full.nix.

{
  programs.bash.enable = true;
}
