{ hostname, desktop, config, lib, ... }:

let
  # Get the hostname prefix from the hostname (e.g. qilin in qilin-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix;

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
