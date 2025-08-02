{ ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
    ../_mixins/suites/photography.nix
  ];

  nixpkgs.config.permittedInsecurePackages = [
    "libsoup-2.74.3"
  ];
}
