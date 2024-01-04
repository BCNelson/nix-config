{ inputs, outputs, hostname, desktop, config, lib, pkgs, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [ ../_mixins/console.nix ]
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  nixpkgs = {
    overlays = [
      inputs.nur.overlay
      outputs.overlays.unstable-packages
    ];
    config= {
      allowUnfreePredicate = _pkg: true;
    };
  };

  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  xdg.enable = true;
  xdg.mime.enable = true;
  targets.genericLinux.enable = true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];

  programs = {
    bash = {
      enable = true;
    };
    git = {
      enable = true;
      userName = "Bradley Nelson";
      userEmail = "bradely@nel.family";
      extraConfig = {
        push = {
          default = "matching";
        };
        pull = {
          rebase = true;
        };
        init = {
          defaultBranch = "main";
        };
      };
    };
  };

  home.packages = [
    #Devtools
    pkgs.git
    pkgs.git-crypt
    pkgs.just
    pkgs.ldns
    pkgs.nmap
  ];

  home.sessionVariables = {
    EDITOR = "vim";
  };
}
