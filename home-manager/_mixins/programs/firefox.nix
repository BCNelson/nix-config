{ pkgs, ... }:
{
  programs.firefox = {
    enable = true;
    package = pkgs.firefox;
    profiles = {
      personal = {
        name = "Personal";
        isDefault = true;
        extensions = {
          packages = with pkgs.nur.repos.rycee.firefox-addons; [
            bitwarden
            darkreader
            google-cal-event-merge
            ublock-origin
            i-dont-care-about-cookies
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
          "datareporting.policy.dataSubmissionPolicyBypassNotification" = true;
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
}
