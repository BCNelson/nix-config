{ platform, inputs, pkgs, ... }:
{
  imports = [
    ./_mixins/selinux.nix
    # Upstream candidate, kept verbatim so it can be lifted into a PR against
    # numtide/system-manager as nix/modules/sysusers.nix. See the README next
    # to it. Until that lands, import it here.
    ../contrib/system-manager-sysusers/module.nix
    ./_mixins/qmk.nix
    ./_mixins/uinput.nix
    ./_mixins/agenix.nix
  ];

  config = {
    nixpkgs.hostPlatform = platform;

    environment.systemPackages = [ pkgs.powertop ];

    nix.enable = true;
    nix.settings = {
      experimental-features = [ "nix-command" "flakes" ];
      auto-optimise-store = true;
      keep-outputs = true;
      keep-derivations = true;
      warn-dirty = false;

      # No channels are registered on system-manager hosts, so pin <nixpkgs>
      # to the same input the system is built from. This makes old-style
      # `nix-shell -p foo` / `nix-shell '<nixpkgs>'` resolve instead of
      # erroring with "file 'nixpkgs' was not found in the Nix search path".
      nix-path = [ "nixpkgs=${inputs.nixpkgs-unstable}" ];

      trusted-users = [ "root" "@wheel" "bcnelson" ];

      extra-substituters = [
        "https://devenv.cachix.org"
        "https://ai.cachix.org"
      ];
      extra-trusted-public-keys = [
        "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw="
        "ai.cachix.org-1:N9dzRK+alWwoKXQlnn0H6aUx0lU/mspIoz8hMvGvbbc="
      ];
    };
  };
}
