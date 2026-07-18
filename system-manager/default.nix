{ platform, inputs, pkgs, ... }:
{
  imports = [ ./_mixins/selinux.nix ];

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
