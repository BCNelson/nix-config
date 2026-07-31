_:

# Tier 2: thin clients only.
#
# You log into one of these to find out why something is broken, not to work on
# it -- and the tools for that (systemctl, journalctl, nix) are already in the
# system profile, not the user's. So this is close to empty on purpose: the
# point is not a slimmed-down workstation, it is the absence of one.
#
# Before adding anything here, check it is not already reachable from the system
# profile, and remember every byte is fetched over the network on each update
# and held twice on the eMMC while the old generation is still around.

{
  # Make it obvious at a glance which class of machine the shell is on, so a
  # stray `nixos-rebuild` habit gets a moment's pause -- the installer tools are
  # not even installed here.
  programs.bash.initExtra = ''
    PS1='\[\e[1;33m\][thin]\[\e[0m\] \u@\h:\w\$ '
  '';
}
