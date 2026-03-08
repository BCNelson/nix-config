{ pkgs, ... }:

{
  programs.zed-editor = {
    enable = true;
    package = pkgs.unstable.zed-editor;
    mutableUserSettings = true;
    userSettings = {
      terminal = {
        shell = {
          program = "fish";
        };
      };
      buffer_font_family = "Monaspace Neon";
      buffer_font_size = 14;
      ui_font_size = 16;
    };
  };
}
