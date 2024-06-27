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
      nixpkgs = inputs.nixpkgs24-05;
      home-manager = inputs.home-manager24-05;
    };
    "23.11" = {
      nixpkgs = inputs.nixpkgs23-11;
      home-manager = inputs.home-manager23-11;
    };
    "24.05" = {
      nixpkgs = inputs.nixpkgs24-05;
      home-manager = inputs.home-manager24-05;
    };
  };

  getSecretWithDefault = path: key: default:
    let
      # This is needed because nix can't import a file that is encrypted https://github.com/NixOS/nix/issues/4329#issuecomment-740787749
      inherit (inputs.nixpkgs.legacyPackages.x86_64-linux) runCommandNoCCLocal file;
      inherit (inputs.nixpkgs.lib) hasInfix fileContents;
      inherit (builtins) pathExists;

      isNotEncrypted = f: hasInfix "text" (fileContents (runCommandNoCCLocal "chk-encryption"
        {
          buildInputs = [ file ];
          src = f;
        } "file $src > $out"));
      hasCredentials = if pathExists path && isNotEncrypted path then true else false;
    in
    if hasCredentials then (import path).${key} else (builtins.trace "${path} is not a nix file does your git-cypt need to be unlocked?" default);
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

  mkDarwin = { hostname, usernames, platform ? "aarch64-darwin", version ? "stable" }: inputs.nix-darwin.lib.darwinSystem {
    specialArgs = {
      inherit inputs outputs hostname usernames;
    };
    modules = [
      ../darwin
      { nixpkgs.hostPlatform = platform; }
      versions.${version}.home-manager.darwinModules.home-manager
      (mkHome { inherit hostname usernames platform; })
    ];
  };

  inherit getSecretWithDefault;

  getSecret = path: key: getSecretWithDefault path key "";

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
