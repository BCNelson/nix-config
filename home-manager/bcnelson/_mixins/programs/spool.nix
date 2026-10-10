# Spool clipboard history (pkgs/spool, modules/home-manager/spool.nix).
#
# Replaces Klipper: takes Meta+V and turns off Klipper's own history. Enabled
# per host from its home config (currently home-manager/bcnelson/sierra.nix).
{outputs, ...}: {
  imports = [outputs.homeModules.spool];

  services.spool = {
    enable = true;
    # Picking an item (Enter) only sets the clipboard; paste it yourself.
    autoPaste = false;
  };
}
