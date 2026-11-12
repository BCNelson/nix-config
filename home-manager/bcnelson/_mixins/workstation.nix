{
  config,
  pkgs,
  inputs,
  ...
}:
let
  # zoom-us is a buildFHSEnv. Its bundled xdg-open sees XDG_CURRENT_DESKTOP=KDE
  # and execs `kde-open`, which isn't in the sandbox -- so "Sign in with SSO"
  # (and every other link) silently opens nothing. Rather than pull KDE
  # Frameworks into the FHS and start a browser inside zoom's namespace, give it
  # a `kde-open` that hands the URL to the desktop portal over the session bus
  # (which the sandbox shares). The host's own default browser opens it, and
  # the zoomus:// redirect comes back through the normal scheme handler.
  #
  # Only URLs: the portal refuses file:// on OpenURI (that's OpenFile, which
  # needs fd passing), so "open recordings folder" style actions still no-op.
  portalOpen = pkgs.writeShellScriptBin "kde-open" ''
    uri="''${1:?usage: kde-open <url>}"
    exec ${pkgs.glib.bin}/bin/gdbus call --session \
      --dest org.freedesktop.portal.Desktop \
      --object-path /org/freedesktop/portal/desktop \
      --method org.freedesktop.portal.OpenURI.OpenURI \
      "" "$uri" "{}" >/dev/null
  '';
  zoom = pkgs.zoom-us.override { targetPkgsFixed = [ portalOpen ]; };
in
{
  imports = [
    ./k8s.nix
    ../../_mixins/programs/libreOffice.nix
    ../../_mixins/programs/vlc.nix
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
    ./programs/trillium.nix
    ./build-cache.nix
    ./lowprio.nix
    ./forgejo-cli.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    (config.lib.nixGL.wrap pkgs.kdePackages.merkuro)
    pkgs.mb4-extractor
    (config.lib.nixGL.wrap zoom)
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
