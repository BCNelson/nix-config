{ pkgs, ... }:
{
    imports = [
        ../docker.nix
    ];

    programs.tmux = {
        enable = true;
        extraConfig = ''
            # Enable mouse
            set -g mode-mouse on
            setw -g mouse-resize-pane on
            setw -g mouse-select-window on
            setw -g mouse-select-pane on

            set -g default-command /usr/local/bin/fish
            set -g default-shell /usr/local/bin/fish
        '';
    };
}