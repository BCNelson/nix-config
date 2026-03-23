{ config, pkgs, lib, desktop, outputs, ... }:

let
  wrappedYakuake = config.lib.nixGL.wrap pkgs.kdePackages.yakuake;
in
{
  # Home Manager needs a bit of information about you and the paths it should
  # manage

  imports = [
    outputs.homeModules.autostart
    ./_mixins/firefox.nix
    ./_mixins/zen.nix
    ../_mixins/programs/chrome.nix
    ../_mixins/programs/vscode.nix
    ../_mixins/programs/zed.nix
  ] ++ lib.optional (builtins.isString desktop && builtins.pathExists ./_mixins/${desktop}.nix) ./_mixins/${desktop}.nix;

  home.packages = [
    wrappedYakuake
    (config.lib.nixGL.wrap pkgs.kdePackages.konsole) # Required for yakuake's terminal KPart component

    # pkgs.quickemu
    # pkgs.quickgui

    (config.lib.nixGL.wrap pkgs.easyeffects)

    (config.lib.nixGL.wrap pkgs.unstable.obsidian)

    (config.lib.nixGL.wrap pkgs.kdePackages.filelight)

    (config.lib.nixGL.wrap pkgs.kdePackages.kate)

    # Chat
    (config.lib.nixGL.wrap pkgs.unstable.discord)

    (config.lib.nixGL.wrap pkgs.newsflash)

    #Dignostic tools
    pkgs.vulkan-tools
    pkgs.libva-utils
  ];

  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    packageSourced = [
      {
        package = wrappedYakuake;
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

  home.sessionVariables = {
    VISUAL = "kwrite";
  };
}
