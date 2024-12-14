{ pkgs, ... }:
{
  imports = [
    ./nvim.nix
  ];

  home.packages = [
    pkgs.winbox4
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
    };
  };
}
