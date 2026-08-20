{
  inputs,
  pkgs,
  ...
}: {
  imports = [
    inputs.zen-browser.homeModules.twilight
  ];

  programs.zen-browser = {
    enable = true;
    profiles = {
      personal = {
        name = "Personal";
        isDefault = true;
        extensions = {
          packages = with pkgs.nur.repos.rycee.firefox-addons; [
            bitwarden
            darkreader
            pay-by-privacy
            google-cal-event-merge
            ublock-origin
            i-dont-care-about-cookies
            languagetool
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
          # Preemptive, not a fix for anything currently broken: DoH is off in
          # this profile (network.trr.mode = 0), so nothing consults it today.
          #
          # It is here because the failure it prevents is genuinely
          # undiagnosable from the browser side. Every nel.family service is
          # split-horizon -- romeo's unbound answers LAN clients with
          # 192.168.3.7 while the public record points at the WAN ingress
          # (66.118.47.137) -- and the router does not do NAT loopback: a LAN
          # host connecting to the public address gets an immediate RST
          # (measured, 2ms). DoH bypasses the OS resolver entirely, so if
          # Mozilla ever flips DoH on by default here, every one of these
          # services starts failing with a browser "CORS Failed" and 0 bytes,
          # which points at exactly the wrong layer.
          #
          # Excluding the domain rather than disabling DoH keeps encrypted DNS
          # for everything else. Suffix match, so it covers *.nel.family.
          #
          # The real fix for the underlying problem is NAT loopback on the
          # router, which would make this irrelevant for every client rather
          # than one browser.
          "network.trr.excluded-domains" = "nel.family";
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
    };
  };
}
