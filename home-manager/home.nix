{ config, pkgs, inputs, outputs, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage.
  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  imports = [
    outputs.homeManagerModules.autostart
    ./firefox.nix
    ./chrome.nix
    ./vscode.nix
    ./deckmaster
  ];

  # This value determines the Home Manager release that your configuration is
  # compatible with. This helps avoid breakage when a new Home Manager release
  # introduces backwards incompatible changes.
  #
  # You should not change this value, even if you update Home Manager. If you do
  # want to update the value, then make sure to first check the Home Manager
  # release notes.
  home.stateVersion = "23.05"; # Please read the comment before changing.

  home.packages = [
    pkgs.yakuake

    #Devtools
    pkgs.git
    pkgs.git-crypt
    pkgs.just


    pkgs.obsidian

    # Chat
    pkgs.neochat
    pkgs.signal-desktop
    pkgs.discord

    pkgs.newsflash
  ];

  programs = {
    fish = {
      enable = true;
      interactiveShellInit = ''
        set fish_greeting # Disable greeting
      '';
      plugins = [
        {
          name = "done";
          src = pkgs.fishPlugins.done.src;
        }
        {
          name = "z";
          src = pkgs.fishPlugins.z.src;
        }
      ];
    };
    git = {
      enable = true;
      userName = "Bradley Nelson";
      userEmail = "bradely@nel.family";
    };
  };

  xdg.enable=true;
  xdg.mime.enable=true;
  targets.genericLinux.enable=true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];
  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    # packages = [ pkgs.yakuake ];
    packageSourced = [ 
      {
        package = pkgs.yakuake;
        path = "share/applications/org.kde.yakuake.desktop";
      }
     ];
  };

  systemd.user.startServices = "sd-switch";

  # Home Manager is pretty good at managing dotfiles. The primary way to manage
  # plain files is through 'home.file'.
  home.file = {
    ".local/share/konsole/Fish.profile".text = ''
    [General]
    Command=~/.nix-profile/bin/fish
    Name=Fish
    Parent=FALLBACK/
    '';
  };

  home.sessionVariables = {
    EDITOR = "vim";
    VISUAL = "kwrite";
    SSH_AUTH_SOCK = "/run/user/1000/gnupg/S.gpg-agent.ssh";
    SSH_AGENT_PID = "";
  };
  # Let Home Manager install and manage itself.
  programs.home-manager.enable = true;
}
