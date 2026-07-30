{ thinClient ? false, lib, ... }:

# The user's console environment, in three tiers.
#
#   common.nix  every user, every machine -- the floor
#   thin.nix    thin clients only
#   full.nix    full-sized hosts only
#
# The split exists because the two classes of machine want opposite things. A
# workstation wants the whole modern-unix toolkit; a Wyse 3040 with 2 GB of RAM
# and an 8 GB eMMC cannot afford it -- full.nix measures ~4.7 GiB per user,
# larger than the entire rest of a thin client's system closure.
#
# `thinClient` comes from lib/default.nix, which derives it from membership of
# hosts/thin-clients.nix and threads it through extraSpecialArgs alongside
# `desktop` and `hostname`.
#
# Deciding where something belongs: does a login on an appliance need it? Then
# common.nix. Only useful when you are actually working on the machine? full.nix.

{
  imports = [ ./common.nix ]
    ++ lib.optional thinClient ./thin.nix
    ++ lib.optional (!thinClient) ./full.nix;
}
