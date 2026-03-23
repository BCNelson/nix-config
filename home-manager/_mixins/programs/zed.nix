{ config, pkgs, ... }:

{
  programs.zed-editor = {
    enable = true;
    package = config.lib.nixGL.wrap pkgs.unstable.zed-editor;
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
      language_models = {
        ollama = {
          api_url = "http://romeo.b.nel.family:11434";
          auto_discover = true;
        };
      };
    };
  };
}
