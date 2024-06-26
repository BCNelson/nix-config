{ hostname, desktop, config, lib, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [ ../_mixins/console.nix ]
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  home.username = "hlnelson";
  home.homeDirectory = "/home/hlnelson";

  xdg.enable = true;
  xdg.mime.enable = true;
  targets.genericLinux.enable = true;
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
