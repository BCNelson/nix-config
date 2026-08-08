{ hostname, desktop, thinClient ? false, config, lib, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [ ../_mixins/console ]
    # This used to arrive via _mixins/console/full.nix, which every user shared.
    # bcnelson moved to herdr and dropped it from there, so it is imported here
    # to keep this user's shell exactly as it was. The thinClient guard mirrors
    # full.nix's own dispatch, which is what gated it before.
    ++ lib.optional (!thinClient) ../_mixins/programs/tmux.nix
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  home.username = "hlnelson";
  home.homeDirectory = "/home/hlnelson";

  xdg.enable = true;
  xdg.mime.enable = true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];

  programs = {
    bash = {
      enable = true;
    };
  };

  home.packages = [ ];

  home.sessionVariables = {
    EDITOR = "vim";
  };
}
