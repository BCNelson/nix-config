{ config, pkgs, ... }:

{
    programs.vscode = {
      enable = true;
      package = pkgs.vscode; #pkgs.vscode.fhs Breaks ssh agent issue #2
      userSettings = {
        "terminal.integrated.defaultProfile.linux" = "fish";
        "workbench.colorTheme" = "Default Dark Modern";
        "editor.inlineSuggest.enabled" = true;
        "github.copilot.enable" = ''{"*": true,}'';
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
}
