{ pkgs, lib, config, ... }:

{
  home.packages = [
    pkgs.distrobox
    pkgs.awscli2
    pkgs.slack
    pkgs.distrobox-bazel # provides bazel/bazelisk wrappers that call into distrobox container
    pkgs.mongodb-compass
    pkgs.hoppscotch
    pkgs.dive
    pkgs.bazel-buildtools
    # pkgs.zed-editor
    pkgs.cloudflared
    pkgs.zoom-us
    pkgs.pulumi
  ];

  programs.firefox = {
    enable = true;
    profiles = {
      personal = {
        isDefault = lib.mkForce false;
      };
      redo = {
        name = "Redo";
        isDefault = true;
        id = 1;
        extensions = {
          packages = with pkgs.nur.repos.rycee.firefox-addons; [
            darkreader
            google-cal-event-merge
            ublock-origin
            i-dont-care-about-cookies
            languagetool
            onepassword-password-manager
            react-devtools
            refined-github
            firefox-color
            stylus
          ];
          force = true;
        };
        search = {
          default = "google";
          force = true;
        };
        settings = {
          # Disable the builtin Password manager
          "signon.rememberSignons" = false;
          "signon.rememberSignons.visibilityToggle" = false;
          "trailhead.firstrun.didSeeAboutWelcome" = true;
          "browser.newtabpage.activity-stream.feeds.section.topstories" = false;
          "browser.newtabpage.activity-stream.showSponsoredTopSites" = false;
          "browser.newtabpage.activity-stream.topSitesRows" = 3;
          "extensions.formautofill.creditCards.enabled" = false;
          "widget.use-xdg-desktop-portal.file-picker" = 1;
        };
      };
    };
    policies = {
      DisablePocket = true;
      DisableSetDesktopBackground = true;
    };
  };




  xdg = {
    desktopEntries = {
      firefox-personal = {
        name = "Firefox (Personal)";
        genericName = "Web Browser";
        exec = "firefox -P \"Personal\"";
        terminal = false;
        categories = [ "Application" "Network" "WebBrowser" ];
      };
    };
    configFile = {
      "distrobox/redo.ini" = {
        enable = true;
        text = ''
          [redo]
          image=ubuntu:latest
          additional_packages="build-essential curl"
          init_hooks="curl -fsSL https://github.com/bazelbuild/bazelisk/releases/latest/download/bazelisk-linux-amd64 -o /usr/local/bin/bazelisk && chmod +x /usr/local/bin/bazelisk && ln -sf /usr/local/bin/bazelisk /usr/local/bin/bazel"
        '';
      };
      # Fish completions for bazel - generated at runtime since bazelisk needs network
      # Run: bazelisk completion fish > ~/.config/fish/completions/bazel.fish
      # to manually generate completions
    };
  };

  systemd.user.services.distrobox-redo-setup = {
    Unit = {
      Description = "Ensure distrobox redo container exists";
      After = [ "docker.service" ];
    };
    Service = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${pkgs.writeShellScript "distrobox-redo-check" ''
        set -e
        INI_FILE="${config.xdg.configHome}/distrobox/redo.ini"

        # Check if container exists
        if ! ${pkgs.distrobox}/bin/distrobox list 2>/dev/null | grep -q "redo"; then
          echo "Container 'redo' not found, creating..."
          ${pkgs.distrobox}/bin/distrobox assemble create --file "$INI_FILE"
        else
          echo "Container 'redo' already exists"
        fi
      ''}";
    };
    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
