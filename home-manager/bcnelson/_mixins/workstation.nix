{ pkgs, ... }:
{
  imports = [
    ./nvim.nix
  ];

  home.packages = [
    pkgs.winbox4
    pkgs.kdePackages.merkuro
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
