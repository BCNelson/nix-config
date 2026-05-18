{ platform, ... }:
{
  imports = [ ./_mixins/selinux.nix ];

  config = {
    nixpkgs.hostPlatform = platform;
  };
}
