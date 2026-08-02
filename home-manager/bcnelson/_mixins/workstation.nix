{
  config,
  pkgs,
  inputs,
  ...
}:
{
  imports = [
    ./nvim.nix
    ./k8s.nix
    ../../_mixins/programs/libreOffice.nix
    ./mcp
    ./mcp/playwrite.nix
    ./mcp/pulumi.nix
    ./claude
    ./claude/skill/pr-review-response
    ./claude/skill/init-devenv
    ./codex
    ./opencode
    ./herdr
    ./happy
    ./programs/trillium.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    (config.lib.nixGL.wrap pkgs.kdePackages.merkuro)
    pkgs.mb4-extractor
    (config.lib.nixGL.wrap pkgs.zoom-us)
    pkgs.gh
    inputs.scaffold.packages.${pkgs.stdenv.hostPlatform.system}.default
    (config.lib.nixGL.wrap pkgs.amazing-marvin)
    (config.lib.nixGL.wrap pkgs.todoist-electron)
    pkgs.yt-dlp
  ];

  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    settings = {
      "*.b.nel.family" = {
        RemoteCommand = "tmux a";
        RequestTTY = "yes";
      };
      "ryuu.llp.nel.family" = {
        RemoteCommand = "tmux a";
        RequestTTY = "yes";
      };
      "vor.ck.nel.family" = {
        RemoteCommand = "tmux a";
        RequestTTY = "yes";
      };
      # git.bcnelson.dev publishes both A and AAAA, but port 22 only answers over
      # IPv4 (80/443 work on both families). Without this, ssh tries the AAAA
      # first and stalls until the connect timeout before falling back, adding
      # over a minute to every git fetch/push. Drop this once IPv6 :22 is
      # unblocked upstream of the host — whiskey's own firewall already allows 22
      # on both families, so the block is in the network in front of it.
      "git.bcnelson.dev" = {
        AddressFamily = "inet";
      };
    };
  };
}
