# The spoold systemd *user* unit, shared by the home-manager module
# (modules/home-manager/spool.nix) and the VM test (./nixos-test.nix) so the
# test exercises exactly the hardening that ships.
#
# Returns raw systemd keys: `unit` ([Unit]), `service` ([Service]), `path`
# (the PATH value; callers put it into Environment= in whatever way their
# module system wants, because NixOS units inject their own PATH) and
# `namespaced` (directives that were evaluated and deliberately NOT shipped;
# the VM test applies them to a probe unit to keep the evidence current).
#
# Findings from the spool-test VM (NixOS unstable, systemd user manager,
# 2026-10-09); nixos-test.nix asserts each of these:
#
# * Namespace directives (ProtectSystem=, ReadWritePaths=, PrivateNetwork=,
#   PrivateTmp=, ProtectHome=, ...) in a *user* unit make systemd set up an
#   unprivileged user namespace, i.e. they imply PrivateUsers=yes. That is
#   fatal for spoold: its peer check stats /proc/<peer>/root (Flatpak
#   detection), which needs ptrace-read access, and the kernel's commoncap
#   check refuses that across user namespaces even for the same uid. With
#   them every client (spoolctl, the picker, KWin's script callbacks) was
#   rejected: "cannot inspect root of peer pid N: Permission denied". So they
#   are left out. What they would have bought is mostly covered by seccomp
#   (RestrictAddressFamilies=AF_UNIX instead of PrivateNetwork=).
#   Also seen in that probe: with ProtectSystem=strict + PrivateNetwork=yes
#   the session bus (%t/bus, incl. org.freedesktop.secrets) and the Wayland
#   socket still connect (path-based AF_UNIX sockets ignore network
#   namespaces), so D-Bus/Wayland were never the problem. And
#   ProtectSystem=strict in this user unit left $HOME and %t writable anyway,
#   so it would have bought little.
# * The seccomp-based directives (SystemCallFilter=, MemoryDenyWriteExecute=,
#   RestrictAddressFamilies=, RestrictNamespaces=, RestrictRealtime=,
#   RestrictSUIDSGID=, LockPersonality=, SystemCallArchitectures=) need no
#   namespace and work in user units because NoNewPrivileges=yes is set
#   (/proc/<pid>/status shows NoNewPrivs 1, Seccomp 2). D-Bus (Secret
#   Service unlock) and Wayland work under them.
#   MemoryDenyWriteExecute is inherited by the picker (spoold execs it): fine
#   for the Slint software renderer, but a GPU renderer on llvmpipe (Mesa's
#   JIT) would need W+X pages.
# * RuntimeDirectory= is created by the user manager with mode 0700 (spoold
#   insists on 0700 for the socket dir); it is removed when the unit stops and
#   spoold recreates its socket on start.
# * No StateDirectory=: for user units, when the legacy ~/.config/<name>
#   exists (home-manager puts config.toml in ~/.config/spool), systemd makes
#   ~/.local/state/<name> a compatibility symlink to it, and spoold rightly
#   refuses a symlinked state dir. spoold creates $XDG_STATE_HOME/spool itself
#   (0700, own uid, no symlinks).
# * LimitMEMLOCK=256M is accepted (`systemctl --user show` reports it) but is
#   NOT effective: an unprivileged user manager cannot raise a limit above its
#   own hard limit, and the process got 8 MiB (the systemd default). Kept for
#   when the system raises it (NixOS:
#   systemd.services."user@".serviceConfig.LimitMEMLOCK); spool-crypto's
#   mlock is best-effort, so 8 MiB only means fewer pages are pinned.
{
  lib,
  package,
  pickerPackage ? null,
}: {
  # Explicit PATH: spoold audits PATH at startup and refuses any entry that is
  # not root-owned (or in /nix/store) or is group/world-writable; an *empty*
  # entry (e.g. PATH="") is also refused. The user manager's PATH typically
  # carries ~/.local/bin and friends (Plasma imports the login environment),
  # so never inherit it. spoold's own bin dir keeps the value non-empty when
  # there is no picker.
  path = lib.makeBinPath ([package] ++ lib.optional (pickerPackage != null) pickerPackage);

  unit = {
    Description = "Spool clipboard history daemon";
    PartOf = ["graphical-session.target"];
    After = ["graphical-session.target"];
  };

  service = {
    Type = "simple";
    # The package's spoold itself, never a wrapper script: spoold finds its
    # KWin script (../share/spool/kwin-script) and the spool-paster helper
    # (same bin dir) relative to its own exe. Auto-paste: KWin (6.7) grants
    # org_kde_kwin_fake_input only to a process whose real /proc/<pid>/exe
    # matches the Exec= of a desktop file declaring
    # X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input. spoold is
    # non-dumpable, so KWin can never read its exe; the grant goes to the
    # dumpable, secret-free helper instead: the package ships
    # share/applications/dev.bcnelson.spool.paster.desktop with
    # Exec=<out>/bin/spool-paster, and the home-manager module puts the
    # package in home.packages so that file is on XDG_DATA_DIRS.
    # (dev.bcnelson.spool.daemon.desktop only names the portal app id.)
    ExecStart = "${package}/bin/spoold";
    Restart = "on-failure";
    RestartSec = "2s";
    Slice = "session.slice";

    UnsetEnvironment = "LD_PRELOAD LD_LIBRARY_PATH";
    UMask = "0077";
    LimitCORE = "0";
    LimitMEMLOCK = "256M";

    RuntimeDirectory = "spool";
    RuntimeDirectoryMode = "0700";

    NoNewPrivileges = "yes";
    RestrictAddressFamilies = "AF_UNIX";
    MemoryDenyWriteExecute = "yes";
    SystemCallFilter = "@system-service";
    SystemCallArchitectures = "native";
    LockPersonality = "yes";
    RestrictNamespaces = "yes";
    RestrictRealtime = "yes";
    RestrictSUIDSGID = "yes";
  };

  # NOT applied to spoold (they imply PrivateUsers=yes in a user unit, which
  # breaks the peer check; see the header). The VM test runs a probe with
  # these to keep that finding verified.
  namespaced = {
    ProtectSystem = "strict";
    ReadWritePaths = "%S/spool %t/spool";
    PrivateNetwork = "yes";
  };
}
