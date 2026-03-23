{ config, pkgs, ... }:
{
  imports = [
    ./nvim.nix
    ./k8s.nix
    ../../_mixins/programs/libreOffice.nix
    ./claude
    ./claude/mcp/playwrite.nix
    ./claude/mcp/pulumi.nix
    ./claude/skill/pr-review-response
    ./claude/skill/init-devenv
    ./programs/trillium.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    (config.lib.nixGL.wrap pkgs.kdePackages.merkuro)
    pkgs.mb4-extractor
    (config.lib.nixGL.wrap pkgs.zoom-us)
    pkgs.gh
    (config.lib.nixGL.wrap pkgs.amazing-marvin)
    (config.lib.nixGL.wrap pkgs.todoist-electron)
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
