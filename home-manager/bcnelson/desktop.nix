{ pkgs, lib, desktop, outputs, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage

  imports = [
    outputs.homeManagerModules.autostart
    ./_mixins/firefox.nix
    ../_mixins/programs/chrome.nix
    ../_mixins/programs/vscode.nix
  ] ++ lib.optional (builtins.isString desktop && builtins.pathExists ./_mixins/${desktop}.nix) ./_mixins/${desktop}.nix;

  home.packages = [
    pkgs.kdePackages.yakuake

    # pkgs.quickemu
    # pkgs.quickgui

    pkgs.helvum
    pkgs.easyeffects

    pkgs.unstable.obsidian

    pkgs.kdePackages.filelight

    pkgs.kdePackages.kate

    # Chat
    pkgs.unstable.discord

    pkgs.newsflash

    pkgs.jellyfin-media-player

    #Dignostic tools
    pkgs.glxinfo
    pkgs.vulkan-tools
    pkgs.libva-utils

    pkgs.solaar
  ];

  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    packageSourced = [
      {
        package = pkgs.kdePackages.yakuake;
        path = "share/applications/org.kde.yakuake.desktop";
      }
    ];
  };

  services.kdeconnect = {
    enable = true;
    indicator = true;
  };

  # Workaround for Failed to start unit kdeconnect-indicator.service: Unit tray.target not found.
  # - https://github.com/nix-community/home-manager/issues/2064
  systemd.user.targets.tray = {
    Unit = {
      Description = "Home Manager System Tray";
      Requires = [ "graphical-session-pre.target" ];
    };
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
