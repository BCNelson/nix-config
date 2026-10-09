# Spool clipboard history (pkgs/spool, modules/home-manager/spool.nix).
#
# Prepared but deliberately not imported anywhere: enabling it replaces
# Plasma's Meta+V and starts recording the clipboard, so rolling it out (first
# to sierra-2) is a decision for whoever owns the desktop. To enable, add
# `./_mixins/programs/spool.nix` to the imports in home-manager/bcnelson/desktop.nix
# (or to a host-specific home config for a single machine).
{outputs, ...}: {
  imports = [outputs.homeModules.spool];

  services.spool.enable = true;
}
