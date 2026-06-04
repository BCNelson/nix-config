{ platform, ... }:
{
  imports = [ ./_mixins/selinux.nix ];

  config = {
    nixpkgs.hostPlatform = platform;

    nix.enable = true;
    nix.settings = {
      experimental-features = [ "nix-command" "flakes" ];
      auto-optimise-store = true;
      keep-outputs = true;
      keep-derivations = true;
      warn-dirty = false;

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
