{ pkgs, lib, ... }:

{
  home.packages = [
    pkgs.distrobox
    pkgs.awscli2
    pkgs.slack
    pkgs.bazelisk
    pkgs.mongodb-compass
    pkgs.hoppscotch
    pkgs.dive
    pkgs.bazel-buildtools
    # pkgs.zed-editor
    pkgs.cloudflared
    pkgs.zoom-us
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
        };
        search = {
          default = "Google";
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

  programs.fish = {
    enable = true;
    shellAliases = {
      bazelisk = "distrobox enter redo -- bazelisk";
    };
  };

  xdg.desktopEntries = {
    firefox-personal = {
      name = "Firefox (Personal)";
      genericName = "Web Browser";
      exec = "firefox -P \"Personal\"";
      terminal = false;
      categories = [ "Application" "Network" "WebBrowser" ];
    };
  };
}
