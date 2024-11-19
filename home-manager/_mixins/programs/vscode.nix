{ pkgs, ... }:

{
  programs.vscode = {
    enable = true;
    package = pkgs.unstable.vscode.fhs; #pkgs.vscode.fhs Breaks ssh agent issue #2
    userSettings = {
      "terminal.integrated.defaultProfile.linux" = "fish";
      "workbench.colorTheme" = "Default Dark Modern";
      "editor.inlineSuggest.enabled" = true;
      "github.copilot.enable" = {
        "*" = true;
      };
      "editor.fontFamily" = "'Monaspace Neon', 'monospace', monospace";
      "update.mode" = "none";
    };
  };

  home.file = {
    codeConfig = {
      enable = false;
      target = ".config/code-flags.conf";
      text = ''
            '';
    };
  };

  programs.fish.functions = {
    zcode = {
      body = ''
        set -l path (z -e $argv)
        direnv exec $path code $path
      '';
    };
  };
}
