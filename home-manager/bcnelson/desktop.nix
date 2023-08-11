{ config, pkgs, outputs, stateVersion, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage.
  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  nixpkgs = {
    overlays = [
      outputs.overlays.unstable-packages
    ];
  };

  imports = [
    outputs.homeManagerModules.autostart
    ./_mixins/programs/firefox.nix
    ./_mixins/programs/chrome.nix
    ./_mixins/programs/vscode.nix
  ];

  home.packages = [
    pkgs.yakuake

    pkgs.quickemu
    pkgs.quickgui


    pkgs.unstable.obsidian

    # Chat
    pkgs.neochat
    pkgs.unstable.signal-desktop
    pkgs.unstable.discord

    pkgs.newsflash

    pkgs.jellyfin-media-player
  ];

  xdg.enable = true;
  xdg.mime.enable = true;
  targets.genericLinux.enable = true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];
  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    packageSourced = [
      {
        package = pkgs.yakuake;
        path = "share/applications/org.kde.yakuake.desktop";
      }
      {
        package = pkgs.neochat;
        path = "share/applications/org.kde.neochat.desktop";
      }
    ];
  };

  services.kdeconnect = {
    enable = true;
    indicator = true;
  };

  services.syncthing.enable = true;

  systemd.user.startServices = "sd-switch";

  # Home Manager is pretty good at managing dotfiles. The primary way to manage
  # plain files is through 'home.file'.
  home.file = {
    ".local/share/konsole/Fish.profile".text = ''
      [General]
      Command=~/.nix-profile/bin/fish
      Name=Fish
      Parent=FALLBACK/

      [Scrolling]
      HistoryMode=2
    '';
  };

  home.sessionVariables = {
    VISUAL = "kwrite";
  };
}
