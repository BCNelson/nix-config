{ pkgs, ... }:
{
  programs.firefox = {
    enable = true;
    package = pkgs.firefox;
    profiles = {
      personal = {
        name = "Personal";
        isDefault = true;
        bookmarks = {
          force = true;
          settings = [
            {
              name = "Waterford";
              url = "https://my.waterford.org/student-home";
            }
          ];
        };
        extensions.force = true;
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
      default = {
        isDefault = false;
        id = 999;
        extensions.force = true;
      };
    };
    policies = {
      DisablePocket = true;
      DisableSetDesktopBackground = true;
      WebsiteFilter = {
        Block = [ "<all_urls>" ];
        Exceptions = [ 
          "https://*.waterford.org/*"
          "https://*.waterfordlabs.org/*"
          "https://*.sentry.io/*"
          "https://www.google.com/recaptcha/*"
        ];
      };
    };
  };
}
