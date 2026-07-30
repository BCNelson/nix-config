{ inputs, outputs, stateVersion, ... }:
let
  # Hosts too small to build or evaluate their own closure. Threaded through to
  # both the NixOS and home-manager module trees as `thinClient`, so config can
  # branch on it the same way it already branches on `desktop` -- see
  # docs/thin-clients.md ("Branching on host class").
  thinClients = import ../hosts/thin-clients.nix;
  isThinClient = hostname: builtins.elem hostname thinClients;

  mkHome = { hostname, usernames, desktop ? null, thinClient ? false, genericLinux ? false, platform ? "x86_64-linux", ... }: {
    home-manager.useGlobalPkgs = false;
    # Thin clients set nix.settings.max-jobs = 0 -- they must never compile
    # anything, because a build there means the closure was not published
    # properly and there is not the RAM to attempt it. But home-manager's
    # default activation realises a `user-environment` derivation locally, and
    # a symlink tree like that can never be substituted from a cache, so
    # home-manager-<user>.service failed on every boot with
    #   error: Cannot build '...-user-environment.drv'
    #          Reason: local builds are disabled (max-jobs = 0)
    #
    # useUserPackages routes home.path through users.users.<name>.packages
    # instead, so it is part of the system closure the builder produces and
    # activation has nothing left to build. Left off elsewhere to keep every
    # other host's behaviour exactly as it was.
    home-manager.useUserPackages = thinClient;
    # Move pre-existing unmanaged files aside instead of aborting activation.
    # Without this a single stale file (e.g. a Firefox-written profiles.ini)
    # fails the user's home-manager unit, which fails switch-to-configuration,
    # which makes auto-update report a failed rebuild every interval.
    home-manager.backupFileExtension = "bak";
    home-manager.extraSpecialArgs = {
      inherit inputs outputs stateVersion desktop hostname platform thinClient genericLinux;
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
      inherit (inputs.nixpkgs-unstable.legacyPackages.x86_64-linux) runCommandLocal file;
      inherit (inputs.nixpkgs-unstable.lib) hasInfix fileContents;
      inherit (builtins) pathExists;

      isNotEncrypted = f: hasInfix "text" (fileContents (runCommandLocal "chk-encryption"
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
  mkHost = { hostname, usernames, desktop ? null, nixosMods ? null, channelName ? "nixpkgs-unstable" }:
    let thinClient = isThinClient hostname; in {
    inherit channelName;
    specialArgs = {
      inherit inputs hostname usernames desktop stateVersion thinClient;
      inherit outputs;
      libx = { inherit getSecretWithDefault getSecret forAllSystems mkHome createDockerComposeStackPackage; };
    };
    modules = [
      ../nixos
      # Always use home-manager-unstable regardless of nixpkgs version
      inputs.home-manager-unstable.nixosModules.home-manager
      (mkHome { inherit hostname usernames desktop thinClient; })
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

  mkStandaloneHome = { hostname, username, desktop ? null, platform ? "x86_64-linux" }:
    inputs.home-manager-unstable.lib.homeManagerConfiguration {
      pkgs = inputs.nixpkgs-unstable.legacyPackages.${platform};
      extraSpecialArgs = {
        inherit inputs outputs stateVersion desktop hostname platform;
        # Standalone home-manager targets non-NixOS machines: not thin clients,
        # and the one case where the genericLinux GL/loader shims are correct.
        thinClient = false;
        genericLinux = true;
      };
      modules = [
        (import ../home-manager { inherit username; })
        {
          targets.genericLinux.nixGL = {
            inherit (inputs.nixgl) packages;
            defaultWrapper = "mesa";
          };
        }
      ];
    };

  mkSystemManager = { hostname, usernames ? [], desktop ? null, modules ? [], platform ? "x86_64-linux" }:
    let
      hostModule = ../system-manager/${hostname}.nix;
      homeModules = inputs.nixpkgs-unstable.lib.optionals (usernames != []) [
        inputs.home-manager-unstable.nixosModules.home-manager
        # system-manager also targets non-NixOS distros, so it wants the shims.
        (mkHome { inherit hostname usernames desktop platform; genericLinux = true; })
        {
          home-manager.sharedModules = [
            {
              targets.genericLinux.nixGL = {
                inherit (inputs.nixgl) packages;
                defaultWrapper = "mesa";
              };
            }
          ];
        }
      ];
    in
    inputs.system-manager.lib.makeSystemConfig {
      specialArgs = {
        inherit inputs outputs hostname platform stateVersion;
      };
      modules = [
        ../system-manager
      ]
      ++ inputs.nixpkgs-unstable.lib.optional (builtins.pathExists hostModule) hostModule
      ++ homeModules
      ++ modules;
    };

  inherit getSecretWithDefault getSecret forAllSystems;
}
