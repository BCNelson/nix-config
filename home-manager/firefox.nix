{ config, pkgs, inputs, ... }:
{
    programs.firefox = {
        enable = true;
        package = pkgs.firefox-wayland;
        profiles = {
            personal = {
                name = "Personal";
                isDefault = true;
                extensions = with pkgs.nur.repos.rycee.firefox-addons; [
                    bitwarden
                    darkreader
                    wallabagger
                ];
                search = {
                    default = "Google";
                    force = true;
                };
                settings = {
                    # Disable the builtin Password manager
                    "signon.rememberSignons" = false;
                    "signon.rememberSignons.visibilityToggle" = false;
                    "trailhead.firstrun.didSeeAboutWelcome"  =  true
                };
            };
        };
    };
}
