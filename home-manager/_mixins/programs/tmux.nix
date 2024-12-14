{pkgs, ...}:
{
  programs.tmux = {
    enable = true;
    newSession = true;
    mouse = true;
    shell = "${pkgs.fish}/bin/fish";
  };
}
