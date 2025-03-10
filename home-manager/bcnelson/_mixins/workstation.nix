{ pkgs, ... }:
{
  imports = [
    ./nvim.nix
  ];

  home.packages = [
    pkgs.winbox4
    pkgs.kdePackages.merkuro
    pkgs.mb4-extractor
    pkgs.zoom-us
    pkgs.claude-code
  ];

  programs.ssh = {
    enable = true;
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
