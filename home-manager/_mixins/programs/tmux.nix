{pkgs, ...}:
{
  programs.tmux = {
    enable = true;
    newSession = true;
    mouse = true;
    shell = "${pkgs.fish}/bin/fish";
    terminal = "screen-256color";
    extraConfig = ''
      set -g set-clipboard on
    '';
  };
}
