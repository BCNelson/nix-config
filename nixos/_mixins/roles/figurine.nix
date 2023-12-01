{pkgs, ...}:
let
    figurine = pkgs.buildGoModule {
        pname = "figurine";
        version = "1.3.0";

        src = pkgs.fetchFromGitHub {
            owner = "arsham";
            repo = "figurine";
            rev = "v1.3.0";
            hash = "sha256-1q6Y7oEntd823nWosMcKXi6c3iWsBTxPnSH4tR6+XYs=";
        };

        vendorHash = "sha256-mLdAaYkQH2RHcZft27rDW1AoFCWKiUZhh2F0DpqZELw=";
    };
in
{
    programs.fish = {
        enable = true;
        loginShellInit = ''
            if set -q SSH_CONNECTION; ${ figurine }/bin/figurine "$HOSTNAME";end
        '';
    };
}