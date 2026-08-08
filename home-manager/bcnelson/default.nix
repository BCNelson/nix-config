{ hostname, desktop, thinClient ? false, config, lib, pkgs, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [
    ../_mixins/console
  ]
  # herdr replaced tmux for this user, and it is the session persistence on
  # every machine now -- not just the ones worked at directly. `herdr --remote
  # <target>` starts a herdr server on the far end, so a box is only reachable
  # that way if it carries herdr's config too. Thin clients are the exception:
  # they keep tmux (see nixos/common.nix) because a login there is for reading a
  # journal, not for holding a session.
  ++ lib.optional (!thinClient) ./_mixins/herdr/core.nix
  ++ lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
  ++ lib.optional (builtins.pathExists ./${hostname}.nix) ./${hostname}.nix
  ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  xdg.enable = true;
  xdg.mime.enable = true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];

  programs = {
    bash = {
      enable = true;
    };
    git = {
      enable = true;
      lfs.enable = true;
      settings = {
        user = {
          name = "Bradley Nelson";
          email = "bradley@nel.family";
        };
        push = {
          default = "current";
        };
        init = {
          defaultBranch = "main";
        };
      };
    };
  };

  home.packages = lib.optionals (!thinClient) [
    #Devtools
    pkgs.git
    pkgs.git-crypt
    pkgs.just
    pkgs.ldns
    pkgs.nmap

    pkgs.hw-probe

    pkgs.lsof

    pkgs.usbutils # lsusb
    pkgs.pciutils # lspcigit
    pkgs.inotify-info
  ];

  home.sessionVariables = {
    EDITOR = "vim";
  };

  home.sessionVariablesExtra = ''
    unset QT_PLUGIN_PATH
  '';
}
