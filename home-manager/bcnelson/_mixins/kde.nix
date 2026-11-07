{ config, inputs, lib, pkgs, ... }:
let
  # Spool (programs/spool.nix) replaces Klipper. In Plasma 6 Klipper lives
  # only in the tray's Clipboard entry (org.kde.plasma.clipboard loads
  # libklipperplugin), so disabling that entry, as "System Tray Settings ->
  # Entries -> Clipboard: Disabled" does, stops Klipper entirely: no second
  # clipboard manager touching the selection (e.g. its empty
  # x-kde-onlyReplaceEmpty offer after unlocking).
  spoolEnabled = (config.services.spool or { }).enable or false;
  # Plasma's default tray entries (its extraItems on this setup, 2026-10).
  # Listing them in knownItems keeps plasmashell from re-enabling the
  # default-on Clipboard entry; a new default entry Plasma ships later is
  # still added by plasmashell itself (it is not "known").
  defaultTrayItems = [
    "org.kde.plasma.manage-inputmethod"
    "org.kde.plasma.clipboard"
    "org.kde.plasma.cameraindicator"
    "org.kde.merkuro.contact.applet"
    "org.kde.kdeconnect"
    "org.kde.plasma.keyboardlayout"
    "org.kde.plasma.bluetooth"
    "org.kde.plasma.devicenotifier"
    "org.kde.plasma.notifications"
    "org.kde.plasma.mediacontroller"
    "org.kde.plasma.brightness"
    "org.kde.plasma.battery"
    "org.kde.plasma.keyboardindicator"
    "org.kde.kscreen"
    "org.kde.plasma.weather"
    "org.kde.plasma.volume"
    "org.kde.plasma.printmanager"
    "org.kde.plasma.networkmanagement"
  ];
in
{
  imports = [
    inputs.plasma-manager.homeModules.plasma-manager
  ];

  # Drop-in on the upstream kde-baloo.service (a full unit here would shadow
  # its ExecStart). Upstream already sets CPUWeight=1/IOWeight=1; these make
  # the indexer yield to everything else rather than merely share fairly.
  # IOWeight/IOSchedulingClass only take effect under BFQ or io.cost -- see
  # the udev rule in nixos/sierra/default.nix.
  xdg.configFile."systemd/user/kde-baloo.service.d/limits.conf".text = ''
    [Service]
    Nice=19
    IOSchedulingClass=idle
    # Upstream default is 25% of RAM (~16G here). Most of the extractor's RSS
    # is the mmap'd LMDB index, which reclaims cheaply, so throttle well below.
    MemoryHigh=3G
    MemoryMax=6G
  '';

  programs = {
    plasma = {
      enable = true;
      shortcuts = {
        "kwin"."Window One Desktop to the Left" = "Meta+Ctrl+Shift+Left";
        "kwin"."Window One Desktop to the Right" = "Meta+Ctrl+Shift+Right";
        "plasmashell"."show-on-mouse-pos" = "Meta+V";
        "kwin"."Edit Tiles" = [ ];
        # Ghostty took over both terminals: Meta+T for a window, F12 for the
        # drop-down. F12 is NOT set here -- ghostty registers it itself through
        # the GlobalShortcuts portal (see ./programs/ghostty.nix), and a
        # kglobalaccel binding on the same key would fight it. Clearing
        # yakuake's is what actually frees the key on machines that already
        # have it written into kglobalshortcutsrc.
        "services/com.mitchellh.ghostty.desktop"."_launch" = "Meta+T";
        "yakuake"."toggle-window-state" = [ ];
        # konsole stays one chord away while ghostty is on trial.
        "services/org.kde.konsole.desktop"."_launch" = "Meta+Shift+T";
        "services/org.kde.krunner.desktop"."_launch" = "Meta+Space";
      };
      workspace.theme = "breeze-dark";
      configFile = {
        # "Open Terminal" / "Open Terminal Here" in dolphin and friends. Plasma
        # resolves the service via KTerminalLauncherJob, which reads the
        # X-TerminalArg* keys out of ghostty's .desktop file; TerminalApplication
        # is the older plain-command form some callers still fall back to.
        kdeglobals."General"."TerminalApplication" = "ghostty";
        kdeglobals."General"."TerminalService" = "com.mitchellh.ghostty.desktop";
        # Baloo defaults to all of $HOME, which on sierra meant ~3.4M files
        # under ~/dev and ~/go and a 5.5G index it rewrote at 50-60MB/s, enough
        # to thermally throttle the NVMe. Index only the user-content folders.
        # Changing this list needs `balooctl6 purge` to drop already-indexed
        # paths from the existing index.
        baloofilerc."General"."folders" = lib.concatMapStringsSep "," (d: "${config.home.homeDirectory}/${d}") [
          "Desktop"
          "Documents"
          "Downloads"
          "Music"
          "notes"
          "Pictures"
          "Videos"
        ];
      };
      panels = [{
        floating = true;
        height = 42;
        alignment = "center";
        hiding = "none";
        lengthMode = "fill";
        location = "bottom";
        screen = [0 1 2 3];
        widgets = [
          {
            name = "org.kde.plasma.taskmanager";
            config = {
              showOnlyCurrentScreen = true;
              showOnlyCurrentDesktop = true;
              showOnlyCurrentActivity = true;
              launchers = [ "preferred://browser" ];
            };
          }
          "org.kde.plasma.marginsseparator"
          # The system tray as a raw widget: plasma-manager's `systemTray`
          # wrapper has no way to set knownItems (its `settings` land outside
          # the config group). Without Spool this is what `systemTray` wrote.
          {
            name = "org.kde.plasma.systemtray";
            config.General = {
              hiddenItems = [ "Yakuake" ];
            } // lib.optionalAttrs spoolEnabled {
              extraItems = lib.remove "org.kde.plasma.clipboard" defaultTrayItems;
              knownItems = defaultTrayItems;
            };
          }
          {
            name = "org.kde.plasma.digitalclock";
            config = {
              showSeconds = 2; # 0 = never, 1 = on hover, 2 = always
            };
          }
        ];
      }];
      input.keyboard.numlockOnStartup = "on";
      kwin = {
        edgeBarrier = 0;
      };
      fonts = {
        fixedWidth = {
          family = "Monaspace Neon";
          pointSize = 10;
        };
      };
    };
    konsole = {
      enable = true;
      defaultProfile = "Fish";
      profiles = {
        Fish = {
          command = "${pkgs.fish}/bin/fish";
          font = {
            name = "Monaspace Neon";
            size = 10;
          };
          extraConfig = {
            "Scrolling" = {
              HistoryMode = 2;
            };
          };
        };
      };
    };
  };

  home.packages = [
    pkgs.dolphin-shred
  ];

  # Plasma finds applications through ksycoca, a cache each process (and each
  # thread -- KRunner's app runner has its own) treats as stale only when one
  # of the directories it scanned has a newer mtime than at the last build.
  # It stats those through the profile symlinks, so they resolve into
  # /nix/store, where every mtime is 1: repointing ~/.nix-profile is invisible
  # to it, and a newly installed app stays out of KRunner and the launcher
  # until KRunner restarts or the user logs out.
  #
  # Bump the mtime of a scanned directory that is *not* in the store instead.
  # Every Plasma process then notices on its next lookup (the check is
  # throttled to once per 1.5s) and rebuilds the cache in-process, with its own
  # environment -- so the cache lands under the hash the session reads and is
  # built by the running Plasma's own ksycoca version, whether that is NixOS's
  # or Fedora's. KRunner's app runner does the check itself at the start of
  # each query, so the first query after a switch can still miss a new app and
  # the next one has it.
  #
  # Tested on sierra, 2026-10-31, one change at a time against a running
  # KRunner: a profile flip alone never shows the app; deleting the caches
  # (what NixOS's plasma6 activation does) works once on a freshly started
  # KRunner and then never again, because its file watcher is left on the
  # unlinked inode it still has mmapped; running kbuildsycoca6 from here
  # writes a cache under a different hash (nixpkgs wraps it with extra
  # XDG_DATA_DIRS) that the session never reads. Touching this directory
  # worked every time, including on a KRunner the deletions had wedged.
  #
  # After installPackages, since that is the step that repoints ~/.nix-profile.
  home.activation.touchApplicationsDir = lib.hm.dag.entryAfter [ "installPackages" ] ''
    run ${pkgs.coreutils}/bin/mkdir -p "${config.xdg.dataHome}/applications"
    run ${pkgs.coreutils}/bin/touch "${config.xdg.dataHome}/applications"
  '';
}
