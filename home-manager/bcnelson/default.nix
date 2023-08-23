{ inputs, hostname, desktop, lib, pkgs, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [ ]
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  nixpkgs = {
    overlays = [ inputs.nur.overlay ];
    config.allowUnfreePredicate = _pkg: true;
  };

  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  programs = {
    fish = {
      enable = true;
      interactiveShellInit = ''
        set fish_greeting # Disable greeting
        direnv hook fish | source
      '';
      plugins = [
        {
          name = "done";
          inherit (pkgs.fishPlugins.done) src;
        }
        {
          name = "z";
          inherit (pkgs.fishPlugins.z) src;
        }
      ];
    };
    bash.enable = true;
    git = {
      enable = true;
      userName = "Bradley Nelson";
      userEmail = "bradely@nel.family";
    };
  };

  home.packages = [
    #Devtools
    pkgs.git
    pkgs.git-crypt
    pkgs.just
    pkgs.direnv
  ];

  home.sessionVariables = {
    EDITOR = "vim";
    # SSH_AUTH_SOCK = "/run/user/1000/gnupg/S.gpg-agent.ssh"; # Todo remove this when it should not be needed but it is.
    # SSH_AGENT_PID = "";
  };
}
