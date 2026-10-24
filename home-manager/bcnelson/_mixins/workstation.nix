{
  config,
  pkgs,
  inputs,
  ...
}:
{
  imports = [
    ./k8s.nix
    ../../_mixins/programs/libreOffice.nix
    ./mcp
    ./mcp/playwrite.nix
    ./mcp/pulumi.nix
    ./mcp/rea.nix
    ./claude
    ./claude/skill/pr-review-response
    ./claude/skill/init-devenv
    ./codex
    ./opencode
    ./herdr
    ./happy
    ./programs/trillium.nix
    ./build-cache.nix
    ./lowprio.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    (config.lib.nixGL.wrap pkgs.kdePackages.merkuro)
    pkgs.mb4-extractor
    (config.lib.nixGL.wrap pkgs.zoom-us)
    pkgs.gh
    inputs.scaffold.packages.${pkgs.stdenv.hostPlatform.system}.default
    (config.lib.nixGL.wrap pkgs.todoist-electron)
    pkgs.yt-dlp
  ];

  # These hosts used to force `RemoteCommand = "tmux a"` so that every ssh landed
  # in a persistent session. That is now herdr's job, reached with
  # `herdr --remote <target>` instead of bare ssh.
  #
  # The entries could not simply be left in place: herdr builds its own ssh
  # config for remote attach and Includes ~/.ssh/config *first*, so a matching
  # RemoteCommand would run `tmux a` on the far end and the attach would never
  # get to start a herdr server. Removing them is what makes --remote work, and
  # it has the side effect that plain `ssh <host>` is a plain shell again -- no
  # more `-o RemoteCommand=none` to get one.
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
  };
}
