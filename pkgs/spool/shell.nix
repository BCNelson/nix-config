# Dev shell for Spool. Takes nixpkgs explicitly (NIX_PATH may be unset), so
# enter it through the flake's pinned nixpkgs from the repo root:
#   nix develop --impure --expr 'let p = (builtins.getFlake "git+file:///home/bcnelson/nix-config").inputs.nixpkgs-unstable.legacyPackages.${builtins.currentSystem}; in p.callPackage ./pkgs/spool/shell.nix {}'
# Use git+file:// (tracked files only), not `getFlake (toString ./.)`: a path
# flake copies the gitignored pkgs/spool/target* build dirs into the store.
{
  mkShell,
  callPackage,
  sway,
  wl-clipboard,
  wayland-utils,
  cargo-nextest,
  clippy,
  rustfmt,
  grim,
  wtype,
  kdePackages,
  dbus,
  gnome-keyring,
  wev,
  xdg-desktop-portal,
  foot,
}:
mkShell {
  # cargo, rustc, pkg-config, sqlcipher
  inputsFrom = [(callPackage ./. {}) (callPackage ./picker.nix {})];
  packages = [
    sway # headless test compositor (see README.md)
    wl-clipboard
    wayland-utils # wayland-info
    cargo-nextest
    clippy
    rustfmt
    grim # screenshots of the headless test compositor (spool-picker)
    wtype # virtual-keyboard key injection (spool-picker)
    # nested virtual KWin for spool-kwin/spool-paste tests (SPOOL_KWIN_TESTS=1)
    kdePackages.kwin
    dbus # dbus-run-session, dbus-send
    gnome-keyring # throwaway Secret Service for spool-keys tests (tests/secret-service-harness.sh)
    wev # logs key events received by a focused window
    # M7 non-KWin backends
    foot # toplevels for foreign-toplevel focus tests (SPOOL_WAYLAND_TESTS=1)
    # real GlobalShortcuts portal stack on a private bus + nested KWin
    # (SPOOL_PORTAL_KDE_TESTS=1; binaries are in libexec, found via the store path)
    xdg-desktop-portal
    kdePackages.xdg-desktop-portal-kde
  ];
}
