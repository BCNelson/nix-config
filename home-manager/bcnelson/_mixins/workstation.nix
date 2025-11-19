{ pkgs, ... }:
{
  imports = [
    ./nvim.nix
    ../../_mixins/programs/libreOffice.nix
    ./claude
    ./claude/mcp/playwrite.nix
  ];

  home.packages = [
    pkgs.winbox4
    pkgs.kdePackages.merkuro
    pkgs.mb4-extractor
    pkgs.zoom-us
    pkgs.gh
    pkgs.amazing-marvin
    pkgs.todoist-electron
  ];

  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    matchBlocks = {
      "*.b.nel.family" = {
        extraOptions = {
          RemoteCommand = "tmux a";
          RequestTTY = "yes";
        };
      };
      "ryuu.llp.nel.family" = {
        extraOptions = {
          RemoteCommand = "tmux a";
          RequestTTY = "yes";
        };
      };
      "vor.ck.nel.family" = {
        extraOptions = {
          RemoteCommand = "tmux a";
          RequestTTY = "yes";
        };
      };
    };
  };
}
