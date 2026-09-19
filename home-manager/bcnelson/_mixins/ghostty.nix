{ config, lib, pkgs, ... }:

let
  # nixGL is a no-op on NixOS and the real wrapper on the genericLinux hosts.
  # Ghostty renders through OpenGL, so it needs the same treatment as the other
  # GPU-backed applications here. The wrapper rewrites the .desktop, systemd and
  # D-Bus service files it ships, so single-instance activation keeps working.
  ghostty = config.lib.nixGL.wrap pkgs.ghostty;
in
{
  programs.ghostty = {
    enable = true;
    package = ghostty;

    settings = {
      # Parity with the konsole "Fish" profile this replaced: fish rather than
      # the passwd shell, which on the Fedora hosts is still bash.
      command = "${pkgs.fish}/bin/fish";

      font-family = "Monaspace Neon";
      font-size = 10;

      # konsole ran with HistoryMode=2 (unlimited). Ghostty bounds scrollback by
      # bytes per surface rather than by lines, and allocates lazily, so this is
      # a ceiling and not a reservation. 10x the 10MB default.
      scrollback-limit = 100000000;

      # Replaces yakuake. The quick terminal is Ghostty's own drop-down: it
      # needs Wayland with wlr-layer-shell (KWin has it; X11 sessions do not),
      # and the global: prefix needs the GlobalShortcuts XDG portal, which
      # Plasma has implemented since 5.27. There is no default binding for it.
      keybind = [ "global:f12=toggle_quick_terminal" ];
      quick-terminal-position = "top";
      quick-terminal-size = "50%";
    };
  };

  # A global: keybind is only registered with the portal while Ghostty is
  # running, so something has to start it at login -- otherwise F12 does nothing
  # until the first window is opened by hand. Ghostty ships a user unit for
  # exactly this (ExecStart passes --initial-window=false, so it stays headless
  # until asked for a window) and home-manager installs it, but installing a
  # unit does not enable it. This is the `systemctl --user enable` symlink,
  # written out declaratively; the unit's own [Install] section names the same
  # target. systemd resolves the dependency by unit name, so the home-manager
  # copy under ~/.config/systemd/user -- and its config-change reload trigger --
  # is still what gets loaded.
  xdg.configFile."systemd/user/graphical-session.target.wants/app-com.mitchellh.ghostty.service" =
    lib.mkIf config.programs.ghostty.systemd.enable {
      source = "${ghostty}/share/systemd/user/app-com.mitchellh.ghostty.service";
    };
}
