# Spool clipboard history (pkgs/spool, modules/home-manager/spool.nix).
#
# Replaces Klipper: takes Meta+V and turns off Klipper's own history. Enabled
# per host from its home config (currently home-manager/bcnelson/sierra.nix).
{
  outputs,
  pkgs,
  ...
}: let
  # ClearURLs tracking rules (LGPL-3.0), pinned to a commit of
  # https://gitlab.com/anti-tracking/ClearURLs/rules. To update: take HEAD
  # from `git ls-remote https://gitlab.com/anti-tracking/ClearURLs/rules.git
  # HEAD` and the new hash from the build error (or nix-prefetch-url).
  clearurlsRules = pkgs.fetchurl {
    url = "https://gitlab.com/anti-tracking/ClearURLs/rules/-/raw/11086f40512774dcadef54079f1ba023bfacf940/data.min.json";
    hash = "sha256-syjSpblbGyO6wskTQqje7j+aOCallQv54o4zL/2UzS8=";
  };
in {
  imports = [outputs.homeModules.spool];

  services.spool = {
    enable = true;
    # Picking an item (Enter) only sets the clipboard; paste it yourself.
    autoPaste = false;

    # Lua filters (spool-scripts). A rewritten copy keeps the original as an
    # earlier version (picker: Ctrl+H; `spoolctl versions`).
    scripts = {
      enable = true;
      files."10-strip-tracking.lua".text =
        ''
          local RULES = "${clearurlsRules}"
        ''
        + builtins.readFile ./spool/strip-tracking.lua;
    };
  };
}
