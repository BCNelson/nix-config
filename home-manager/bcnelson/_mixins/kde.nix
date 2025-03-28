{ inputs, pkgs, ... }:
{
  imports = [
    inputs.plasma-manager.homeManagerModules.plasma-manager
  ];

  programs = {
    plasma = {
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
          {
            name = "org.kde.plasma.taskmanager";
            config = {
              showOnlyCurrentScreen = true;
              showOnlyCurrentDesktop = true;
              showOnlyCurrentActivity = true;
              launchers = [ "preferred://browser" ];
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
    konsole = {
      enable = true;
      defaultProfile = "Fish";
      profiles = {
        Fish = {
          command = "${pkgs.fish}/bin/fish";
          extraConfig = {
            "Scrolling" = {
              HistoryMode = 2;
            };
          };
        };
      };
    };
  };

  home.packages = [
    pkgs.dolphin-shred
  ];
  
  catppuccin.kvantum.enable = true;
}
