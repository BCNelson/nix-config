{ inputs, outputs, hostname, desktop, lib, pkgs, ... }:

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
    config.allowUnfreePredicate = _pkg: true;
  };

  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  programs = {
    bash = {
      enable = true;
      bashrcExtra = ''
        # Start fish if we're in an SSH session and have not already started fish
        if [ -z "$STARTEDFISH" ] && [ -n "$SSH_TTY" ];
        then
            export STARTEDFISH=1;
            exec fish;
            exit;
        fi
      '';
    };
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
  ];

  home.sessionVariables = {
    EDITOR = "vim";
  };
}
