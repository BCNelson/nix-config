{ inputs, outputs, stateVersion, ... }: {
  # Helper function for generating home-manager configs
  mkHome = { hostname, username, desktop ? null, platform ? "x86_64-linux" }: inputs.home-manager.lib.homeManagerConfiguration {
    pkgs = inputs.nixpkgs.legacyPackages.${platform};
    extraSpecialArgs = {
      inherit inputs outputs desktop hostname platform username stateVersion;
    };
    modules = [ ../home-manager ];
  };

  # Helper function for generating host configs
  mkHost = { hostname, username, desktop ? null, installer ? null, libx ? null }: inputs.nixpkgs.lib.nixosSystem {
    specialArgs = {
      inherit inputs outputs desktop hostname username stateVersion libx;
    };
    modules = [ ../nixos ] ++ (inputs.nixpkgs.lib.optionals (installer != null) [ installer ]);
  };

  forAllSystems = inputs.nixpkgs.lib.genAttrs [
    "aarch64-linux"
    "i686-linux"
    "x86_64-linux"
    "aarch64-darwin"
    "x86_64-darwin"
  ];

  createDockerComposeStackPackage = {
    name,
    src,
    dockerComposeDefinition,
    platform ? "x86_64-linux"
  }:
  let
     startScript = ''
      #!/usr/bin/env bash
      set -euo pipefail
      pushd %outDir%
      docker-compose "$@"
    '';
    pkgs = inputs.nixpkgs.legacyPackages.${platform};
  in pkgs.stdenv.mkDerivation {
    name = "${name}-docker-stack";
    src = src;
    buildInputs = [ pkgs.docker-compose pkgs.docker];
    installPhase = ''
      cp -r $src/* $out
      mkdir -p $out/bin
      echo "${startScript}" | sed "s+%outDir%+$out+" > $out/bin/dockerStack-${name}
      echo "${builtins.toJSON dockerComposeDefinition}" > $out/docker-compose.yml
    '';
  };

}
