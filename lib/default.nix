{ inputs, outputs, stateVersion, ... }:
let
  mkHome = { hostname, usernames, desktop ? null, platform ? "x86_64-linux", ... }: {
    home-manager.useGlobalPkgs = false;
    home-manager.useUserPackages = false;
    home-manager.extraSpecialArgs = {
      inherit inputs outputs stateVersion desktop hostname platform;
      # pkgs = pkgs.legacyPackages.${platform};
      # inherit (pkgs) lib;
    };
    home-manager.users = builtins.listToAttrs (map
      (username: {
        name = username;
        value = import ../home-manager { inherit username; };
      })
      usernames);
  };

  versions = {
    unstable = {
      nixpkgs = inputs.nixpkgs-unstable;
      home-manager = inputs.home-manager-unstable;
    };
    stable = {
      inherit (inputs) nixpkgs;
      inherit (inputs) home-manager;
    };
    "23.11" = {
      inherit (inputs) nixpkgs;
      inherit (inputs) home-manager;
    };
  };
in
{
  # Helper function for generating host configs
  mkHost = { hostname, usernames, desktop ? null, nixosMods ? null, libx ? null, version ? "stable" }: versions.${version}.nixpkgs.lib.nixosSystem {
    specialArgs = {
      inherit inputs outputs desktop hostname usernames stateVersion libx;
    };
    modules = [
      ../nixos
      versions.${version}.home-manager.nixosModules.home-manager
      (mkHome { inherit hostname usernames desktop; })
    ] ++ (versions.${version}.nixpkgs.lib.optionals (nixosMods != null) [ nixosMods ]);
  };

  getSecret = path: key:
    let
      rawSecret = builtins.readFile path;
      isNix = builtins.substring 0 1 rawSecret == "{";
    in
    if isNix then (import path).${key} else (builtins.trace rawSecret "");

  forAllSystems = inputs.nixpkgs.lib.genAttrs [
    "aarch64-linux"
    "i686-linux"
    "x86_64-linux"
    "aarch64-darwin"
    "x86_64-darwin"
  ];

  createDockerComposeStackPackage =
    { name
    , src
    , dockerComposeDefinition
    , dependencies ? [ ]
    , platform ? "x86_64-linux"
    }:
    let
      startScript = ''
        #!/usr/bin/env bash
        set -euo pipefail
        pushd %outDir%
        echo $PWD
        echo "Command: docker-compose ''$@"
        export COMPOSE_PROJECT_NAME=${name}
        docker compose -f %outDir%/docker-compose.yml ''$@
      '';
      pkgs = inputs.nixpkgs.legacyPackages.${platform};
    in
    pkgs.stdenv.mkDerivation {
      name = "${name}-docker-stack";
      runLocal = true;
      inherit src;
      buildInputs = [ pkgs.docker-compose pkgs.docker ] ++ dependencies;
      installPhase = ''
        echo "Copying files from $src to $out"
        mkdir -p "$out/bin/"
        cp -r $src/. $out/
        echo '${startScript}' | sed "s+%outDir%+$out+" > $out/bin/dockerStack-${name}
        chmod +x $out/bin/dockerStack-${name}
        echo '${builtins.toJSON dockerComposeDefinition}' > $out/docker-compose.yml
      '';
    };

}
