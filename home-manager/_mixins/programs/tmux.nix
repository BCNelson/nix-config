{pkgs, ...}:
{
    programs.tmux = {
        enable = true;
        newSession = true;
        mouse = true;
    };
}