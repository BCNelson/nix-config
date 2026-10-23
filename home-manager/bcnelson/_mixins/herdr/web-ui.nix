{ pkgs, ... }:

# herdr web ui: a browser and phone client for the local herdr session -- every
# workspace and agent pane, chat and live terminal, and New workspace in any
# folder (typed or picked with Browse). It is how agents on this machine are
# started and steered from the phone.
#
# herdr starts it: the manifest's [[startup]] hook brings the server up on
# 127.0.0.1:7317 whenever the herdr server starts, so it is up exactly while
# herdr is. Reaching it from other devices is the host's `tailscale serve`
# (nixos/sierra/herdr-web-ui.nix); access is by Tailscale login, and devices
# with someone else's login pair with a code.
#
# In-app updates cannot happen: the updater works by fetching tags into the
# plugin's own git checkout, and a store path has none, so the version comes
# from pkgs/herdr-web-ui like everything else.

let
  inherit (pkgs) herdr-web-ui;
in
{
  # Registered in plugins.json by ./core.nix.
  local.herdr.pluginRoots = [ "${herdr-web-ui}/${herdr-web-ui.pluginRoot}" ];

  # The plugin reads KEY=value lines from `env` in its config dir
  # (`herdr plugin config-dir devswha.herdr-web-ui`). Takes effect the next
  # time the server starts (the plugin's stop/start actions, or a herdr restart).
  xdg.configFile."herdr/plugins/config/devswha.herdr-web-ui/env".text = ''
    # No anonymous usage counts to herdrweb.dev.
    HERDR_WEB_TELEMETRY=0
  '';
}
