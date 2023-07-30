{ config, pkgs, ... }:

{
    programs.vscode = {
      enable = true;
      package = pkgs.vscode;
      userSettings = {
        "terminal.integrated.defaultProfile.linux" = "fish";
        "workbench.colorTheme" = "Default Dark Modern";
        "editor.inlineSuggest.enabled" = true;
        "github.copilot.enable" = ''{"*": true,}'';
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
