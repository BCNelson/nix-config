{ inputs, pkgs, ... }:
{
  imports = [
    inputs.plasma-manager.homeManagerModules.plasma-manager
  ];
  programs.plasma = {
    enable = true;
    shortcuts = {
      "kwin"."Window One Desktop to the Left" = "Meta+Ctrl+Shift+Left";
      "kwin"."Window One Desktop to the Right" = "Meta+Ctrl+Shift+Right";
      "plasmashell"."show-on-mouse-pos" = "Meta+V";
      "kwin"."Edit Tiles" = [ ];
      "services/org.kde.konsole.desktop"."_launch" = "Meta+T";
      "services/org.kde.krunner.desktop"."_launch" = "Meta+Space";
      "yakuake"."toggle-window-state" = "F12";
    };
    workspace.theme = "breeze-dark";
    configFile = { };
    panels = [{
      floating = true;
      height = 42;
      alignment = "center";
      hiding = "none";
      lengthMode = "fill";
      location = "bottom";
      screen = "all";
      widgets = [
        "org.kde.plasma.kickoff"
        {
          name = "org.kde.plasma.taskmanager";
          config = {
            showOnlyCurrentScreen = true;
            showOnlyCurrentDesktop = true;
            showOnlyCurrentActivity = true;
            launchers = [
              "preferred://browser"
              "file://${pkgs.libreoffice-qt6-still}/share/applications/writer.desktop"
            ];
          };
        }
        "org.kde.plasma.marginsseparator"
        {
          systemTray = {
            items.hidden = [ "Yakuake" ];
          };
        }
        {
          name = "org.kde.plasma.digitalclock";
          config = {
            showSeconds = 2; # 0 = never, 1 = on hover, 2 = always
          };
        }
      ];
    }];
    input.keyboard.numlockOnStartup = "on";
    kwin = {
      edgeBarrier = 0;
    };
    fonts = {
      fixedWidth = {
        family = "Monaspace Neon";
        pointSize = 10;
      };
    };
  };
  catppuccin.kvantum.enable = true;
}
