{ config, pkgs, ... }:

let
  # nixGL is a no-op on NixOS and the GL/Vulkan shim on the genericLinux hosts;
  # ghostty is a GPU-accelerated GTK app, so it needs the same treatment as
  # every other graphical package here. The wrapper rewrites the store paths
  # inside share/applications, share/dbus-1 and share/systemd/user, so the
  # .desktop file and the D-Bus/systemd units below all point at the wrapper
  # rather than the bare package.
  ghosttyPkg = config.lib.nixGL.wrap pkgs.ghostty;
in
{
  programs.ghostty = {
    enable = true;
    package = ghosttyPkg;

    settings = {
      # Same shell and font the konsole "Fish" profile used, so the switch is
      # not also a change of shell or typeface. The login shell is bash (see
      # getent passwd), so fish has to be asked for explicitly.
      command = "${pkgs.fish}/bin/fish";

      # Non-negotiable for a drop-down terminal. Ghostty defaults this to true
      # on Linux, so the whole process exits as soon as the last surface closes
      # -- which for this setup means: exit the shell in the quick terminal and
      # ghostty is gone, taking the portal's F12 registration with it. The next
      # F12 then either does nothing or cold-starts the app (process + GL + font
      # load + shell, several seconds, no slide animation because the surface is
      # being built from scratch). The service exists precisely so one process
      # stays resident and holds that shortcut.
      quit-after-last-window-closed = false;
      font-family = "Monaspace Neon";
      font-size = 10;

      # konsole ran with HistoryMode=2 (unlimited). Ghostty cannot do unlimited
      # yet -- the buffer is in memory and capped in bytes -- so take 100MB per
      # surface instead of the 10MB default. It is allocated lazily.
      scrollback-limit = 100000000;

      # The yakuake replacement. Ghostty's quick terminal is a Wayland
      # layer-shell surface, so this only works in a Wayland session (all the
      # KDE sessions here are).
      #
      # `global:` keybinds on Linux go through the XDG desktop portal's
      # GlobalShortcuts interface, which Plasma has implemented since 5.27.
      # Ghostty has to be *running* to register the shortcut, which is what the
      # systemd unit below is for. The binding shows up in System Settings ->
      # Shortcuts under Ghostty; if the portal ever refuses F12, that is where
      # to re-point it.
      keybind = [ "global:f12=toggle_quick_terminal" ];
      quick-terminal-position = "top";
      quick-terminal-size = "50%";

      # gtk-quick-terminal-layer is deliberately left at its default of `top`.
      # `overlay` was tried here and must not come back: under KWin it makes the
      # surface take several seconds to reach the screen after the toggle, with
      # ghostty idle the whole time (the keypress reaches it in ~1ms via the
      # portal, measured, and it burns no CPU -- it waits). On `top` the same
      # toggle is immediate. The blank-grey-slide that originally motivated
      # `overlay` turned out to be a symptom of quit-after-last-window-closed
      # above, not of the layer.
      # Stays put when focus moves elsewhere, unlike yakuake. This is ghostty's
      # own Linux default and it has to stay false on 1.3.1: with autohide on,
      # toggling the quick terminal off under KWin leaves a blank grey layer
      # surface behind that only goes away by clicking elsewhere, and toggling
      # back on gives the same empty box. Upstream tracked it as
      # ghostty-org/ghostty#11679 and fixed it for 1.3.2, so this can go back to
      # true once the channel carries that.
      quick-terminal-autohide = false;
    };
  };

  # Ghostty is D-Bus activatable, so a normal launch starts it on demand -- but
  # the global F12 shortcut is only registered with the portal while the process
  # is alive, so it has to be up from login rather than from the first window.
  # The home-manager module installs the unit but nothing enables it (it is
  # dropped in via xdg.configFile, not systemd.user.services), so link it into
  # graphical-session.target by hand the same way `systemctl --user enable`
  # would. The unit passes --initial-window=false, so nothing appears on screen.
  xdg.configFile."systemd/user/graphical-session.target.wants/app-com.mitchellh.ghostty.service".source =
    "${ghosttyPkg}/share/systemd/user/app-com.mitchellh.ghostty.service";

  # A dead ghostty is a dead F12, and nothing else notices: the shortcut simply
  # stops working. quit-after-last-window-closed above removes the expected way
  # for that to happen, so this only catches crashes. Separate drop-in file from
  # the home-manager module's own overrides.conf, which owns X-SwitchMethod.
  xdg.configFile."systemd/user/app-com.mitchellh.ghostty.service.d/restart.conf".text = ''
    [Service]
    Restart=on-failure
    RestartSec=2
  '';
}
