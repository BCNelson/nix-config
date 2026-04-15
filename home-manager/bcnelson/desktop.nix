{ config, pkgs, lib, desktops, outputs, ... }:

let
  isKde = builtins.any (d: builtins.elem d [ "kde" "kde5" "kde6" ]) desktops;
  wrappedYakuake = config.lib.nixGL.wrap pkgs.kdePackages.yakuake;
in
{
  imports = [
    outputs.homeModules.autostart
    ./_mixins/firefox.nix
    ./_mixins/zen.nix
    ../_mixins/programs/chrome.nix
    ../_mixins/programs/vscode.nix
    ../_mixins/programs/zed.nix
    ../_mixins/programs/super-productivity.nix
  ] ++ builtins.concatMap (d: lib.optional (builtins.pathExists ./_mixins/${d}.nix) ./_mixins/${d}.nix) desktops;

  home.packages = [
    (config.lib.nixGL.wrap pkgs.easyeffects)

    (config.lib.nixGL.wrap pkgs.unstable.obsidian)

    # Chat
    (config.lib.nixGL.wrap pkgs.unstable.discord)

    (config.lib.nixGL.wrap pkgs.newsflash)

    #Dignostic tools
    pkgs.vulkan-tools
    pkgs.libva-utils
  ] ++ lib.optionals isKde [
    wrappedYakuake
    (config.lib.nixGL.wrap pkgs.kdePackages.konsole) # Required for yakuake's terminal KPart component
    (config.lib.nixGL.wrap pkgs.kdePackages.filelight)
    (config.lib.nixGL.wrap pkgs.kdePackages.kate)
  ];

  programs.bash.enable = true;

  services.freedesktop.autostart = lib.mkIf isKde {
    enable = true;
    packageSourced = [
      {
        package = wrappedYakuake;
        path = "share/applications/org.kde.yakuake.desktop";
      }
    ];
  };

  services.kdeconnect = lib.mkIf isKde {
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

  home.sessionVariables = lib.mkIf isKde {
    VISUAL = "kwrite";
  };
}
