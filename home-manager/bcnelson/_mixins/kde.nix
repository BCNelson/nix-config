{ inputs, ... }:
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
    configFile = { };
  };
  qt.style.catppuccin.enable = true;
}
