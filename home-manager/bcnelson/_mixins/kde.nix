{ inputs, lib, pkgs, ... }:
{
  imports = [
    inputs.plasma-manager.homeModules.plasma-manager
  ];

  programs = {
    plasma = {
      enable = true;
      shortcuts = {
        "kwin"."Window One Desktop to the Left" = "Meta+Ctrl+Shift+Left";
        "kwin"."Window One Desktop to the Right" = "Meta+Ctrl+Shift+Right";
        "plasmashell"."show-on-mouse-pos" = "Meta+V";
        "kwin"."Edit Tiles" = [ ];
        "services/org.kde.konsole.desktop"."_launch" = "Meta+T";
        "services/org.kde.krunner.desktop"."_launch" = "Meta+Space";
        "yakuake"."toggle-window-state" = "F12";
      };
      workspace.theme = "breeze-dark";
      configFile = { };
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
          {
            systemTray = {
              items.hidden = [ "Yakuake" ];
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

  # Plasma finds applications through ksycoca, a cache it only rebuilds when
  # KDirWatch notices a change under one of the XDG application directories.
  # A home-manager switch swaps the whole ~/.nix-profile symlink chain rather
  # than touching those directories, so the watch never fires and a newly
  # installed package's .desktop entry stays invisible to KRunner and the
  # launcher until the next login. Rebuild the cache at the end of activation
  # instead -- it is a ~0.2s directory scan, cheap enough to run every time.
  #
  # Two things make this fiddlier than calling kbuildsycoca6:
  #
  #   * The cache file is named for a hash of XDG_DATA_DIRS plus the language,
  #     so it has to run with the *session's* environment. Invoked from a
  #     `just update-home` shell -- which carries a devshell's XDG_DATA_DIRS --
  #     or over ssh, it would write a differently-hashed cache that the running
  #     session never reads, and silently appear to work.
  #   * ksycoca is versioned, so the builder must match the running Plasma. On
  #     this genericLinux host that is Fedora's /usr/bin/kbuildsycoca6, not
  #     anything from nixpkgs.
  #
  # Lifting PATH and the session variables off plasmashell itself satisfies
  # both, and gives a natural no-op when there is no session to update.
  #
  # After installPackages, since that is the step that repoints ~/.nix-profile
  # and so the step that makes the new .desktop files exist.
  home.activation.rebuildKsycoca = lib.hm.dag.entryAfter [ "installPackages" ] ''
    plasmaPid=$(${pkgs.procps}/bin/pgrep -x -u "$(${pkgs.coreutils}/bin/id -u)" plasmashell | ${pkgs.coreutils}/bin/head -n1 || true)

    if [ -z "$plasmaPid" ] || [ ! -r "/proc/$plasmaPid/environ" ]; then
      verboseEcho "No Plasma session of our own; skipping ksycoca rebuild"
    else
      # Only the variables ksycoca's cache identity and D-Bus notification
      # depend on. Read line-wise off the NUL-separated environ -- none of
      # these ever contain a newline.
      sessionEnv=()
      sessionPath=""
      while IFS= read -r envVar; do
        case "$envVar" in
          PATH=*)
            sessionPath="''${envVar#PATH=}"
            sessionEnv+=("$envVar")
            ;;
          LANG=* | LANGUAGE=* | LC_ALL=* | XDG_DATA_DIRS=* | XDG_DATA_HOME=* | XDG_CURRENT_DESKTOP=* | XDG_RUNTIME_DIR=* | DBUS_SESSION_BUS_ADDRESS=*)
            sessionEnv+=("$envVar")
            ;;
        esac
      done < <(${pkgs.coreutils}/bin/tr '\0' '\n' < "/proc/$plasmaPid/environ")

      # Resolved against the session's PATH so we get the same KDE build that
      # is running, whether that is Fedora's or a NixOS system profile's.
      kbuildsycoca=$(PATH="$sessionPath" command -v kbuildsycoca6 2>/dev/null || true)

      if [ -z "$kbuildsycoca" ]; then
        verboseEcho "kbuildsycoca6 not on the Plasma session's PATH; skipping ksycoca rebuild"
      else
        run ${pkgs.coreutils}/bin/env "''${sessionEnv[@]}" "$kbuildsycoca"
      fi
    fi
  '';
}
