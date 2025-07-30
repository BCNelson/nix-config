{ inputs, outputs, stateVersion, ... }:
let
  mkHome = { hostname, usernames, desktop ? null, platform ? "x86_64-linux", ... }: {
    home-manager.useGlobalPkgs = false;
    home-manager.useUserPackages = false;
    home-manager.extraSpecialArgs = {
      inherit inputs outputs stateVersion desktop hostname platform;
    };
    home-manager.users = builtins.listToAttrs (map
      (username: {
        name = username;
        value = import ../home-manager { inherit username; };
      })
      usernames);
  };


  getSecretWithDefault = path: key: default:
    let
      # This is needed because nix can't import a file that is encrypted https://github.com/NixOS/nix/issues/4329#issuecomment-740787749
      inherit (inputs.nixpkgs-unstable.legacyPackages.x86_64-linux) runCommandNoCCLocal file;
      inherit (inputs.nixpkgs-unstable.lib) hasInfix fileContents;
      inherit (builtins) pathExists;

      isNotEncrypted = f: hasInfix "text" (fileContents (runCommandNoCCLocal "chk-encryption"
        {
          buildInputs = [ file ];
          src = f;
        } "file $src > $out"));
      hasCredentials = if pathExists path && isNotEncrypted path then true else false;
    in
    if hasCredentials then (import path).${key} else (builtins.trace "${path} is not a nix file does your git-cypt need to be unlocked?" default);
  
  getSecret = path: key: getSecretWithDefault path key "";
  
  forAllSystems = inputs.nixpkgs-unstable.lib.genAttrs [
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
      pkgs = inputs.nixpkgs-unstable.legacyPackages.${platform};
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
in
{
  # Helper function for generating flake-utils-plus host configs
  mkHost = { hostname, usernames, desktop ? null, nixosMods ? null, channelName ? "nixpkgs-unstable" }: {
    inherit channelName;
    specialArgs = {
      inherit inputs hostname usernames desktop stateVersion;
      inherit outputs;
      libx = { inherit getSecretWithDefault getSecret forAllSystems mkHome createDockerComposeStackPackage; };
    };
    modules = [
      ../nixos
      # Always use home-manager-unstable regardless of nixpkgs version
      inputs.home-manager-unstable.nixosModules.home-manager
      (mkHome { inherit hostname usernames desktop; })
    ] ++ (if nixosMods != null then [ nixosMods ] else []);
  };


  mkDarwin = { hostname, usernames, platform ? "aarch64-darwin" }: inputs.nix-darwin.lib.darwinSystem {
    specialArgs = {
      inherit inputs outputs hostname usernames;
    };
    modules = [
      ../darwin
      { nixpkgs.hostPlatform = platform; }
      inputs.home-manager-unstable.darwinModules.home-manager
      (mkHome { inherit hostname usernames platform; })
    ];
  };

  inherit getSecretWithDefault getSecret forAllSystems;
}
