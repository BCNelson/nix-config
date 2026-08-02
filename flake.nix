{
  description = "Bcnleson's NixOS configuration";

  inputs = {
    # Flake utils
    flake-utils-plus.url = "github:gytis-ivaskevicius/flake-utils-plus";
    
    # Nixpkgs
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";
    nixpkgs-unstable-small.url = "github:nixos/nixpkgs/nixos-unstable-small";

    # happy-coder PR #492656 — monorepo migration, brings 1.1.x without bundled
    # claude-code. Remove once merged into unstable.
    nixpkgs-happy-coder.url = "github:colonelpanic8/nixpkgs/happy-coder-monorepo-migration";

    # Home manager - always use unstable
    home-manager-unstable.url = "github:nix-community/home-manager/master";
    home-manager-unstable.inputs.nixpkgs.follows = "nixpkgs-unstable";

    agenix.url = "github:ryantm/agenix";
    agenix-rekey.url = "github:oddlama/agenix-rekey";
    agenix-rekey.inputs.nixpkgs.follows = "nixpkgs-unstable";
    agenix-template.url = "github:jhillyerd/agenix-template/1.0.0";

    authentik-nix = {
      url = "github:nix-community/authentik-nix";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    nixvim = {
      url = "github:nix-community/nixvim";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    plasma-manager = {
      url = "github:nix-community/plasma-manager";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
      inputs.home-manager.follows = "home-manager-unstable";
    };

    nix-formatter-pack.url = "github:Gerschtli/nix-formatter-pack";
    nix-formatter-pack.inputs.nixpkgs.follows = "nixpkgs-unstable";

    disko.url = "github:nix-community/disko";
    disko.inputs.nixpkgs.follows = "nixpkgs-unstable";

    nixos-hardware.url = "github:nixos/nixos-hardware";

    # Add the Nix User Repository (NUR)
    nur.url = "github:nix-community/NUR";

    nixos-generators = {
      url = "github:nix-community/nixos-generators";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    lanzaboote = {
      url = "github:nix-community/lanzaboote/v0.4.2";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    nix-darwin.url = "github:LnL7/nix-darwin/master";
    nix-darwin.inputs.nixpkgs.follows = "nixpkgs-unstable";

    nixarr.url = "github:rasmus-kirk/nixarr/dev";

    zen-browser = {
      url = "github:0xc000022070/zen-browser-flake";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
      inputs.home-manager.follows = "home-manager-unstable";
    };

    scaffold = {
      url = "github:BCNelson/ProjectTemplate";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    cadence = {
      url = "github:BCNelson/cadence";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    homefirst-modules = {
      url = "git+https://codeberg.org/home-first/nixos-modules.git";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    system-manager = {
      url = "github:numtide/system-manager";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };

    # Pi extensions are sourced and dependency-built by Nix rather than allowing
    # `pi install` to fetch mutable package contents at runtime.
    pi-mcp-adapter = {
      url = "github:nicobailon/pi-mcp-adapter/2606ec21d70ab0f6d862ecef5bc734c47d44034b";
      flake = false;
    };

    pi-permission-system = {
      url = "github:gotgenes/pi-packages/da9db2864cd40cc7902c2eae2ffccb8f7ac6a2bb";
      flake = false;
    };

    # Third-party browser client for herdr; upstream herdr ships no web UI of
    # its own. Pinned to a release tag because the bridge vendors herdr's
    # private protocol types and only speaks one protocol version - bumping it
    # means checking the release notes against pkgs.herdr. Built by
    # pkgs/herdr-web.
    herdr-web = {
      url = "github:kcosr/herdr-web/v0.4.0";
      flake = false;
    };

    nixgl = {
      url = "github:nix-community/nixGL";
      inputs.nixpkgs.follows = "nixpkgs-unstable";
    };
  };

  outputs = inputs@{ self, flake-utils-plus, ... }:
    let
      libx = import ./lib { inherit inputs; stateVersion = "23.05"; outputs = self; };
    in
    flake-utils-plus.lib.mkFlake {
      inherit self inputs;

      supportedSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];

      channels.nixpkgs.input = inputs.nixpkgs;
      channels.nixpkgs-unstable.input = inputs.nixpkgs-unstable;
      channels.nixpkgs-unstable-small.input = inputs.nixpkgs-unstable-small;
      channels.nixpkgs-unstable-small-patched = {
        input = inputs.nixpkgs-unstable-small;
        patches = [ ./patches/405787.patch ];
      };

      channelsConfig.allowUnfree = true;

      hostDefaults = {
        system = "x86_64-linux";
        modules = [
          inputs.agenix.nixosModules.default
          inputs.agenix-rekey.nixosModules.default
          inputs.agenix-template.nixosModules.default
          inputs.authentik-nix.nixosModules.default
        ] ++ (builtins.attrValues (import ./modules/nixos));
      };

      hosts = {
        # INSERT_NEW_HOST_CONFIG_HERE
        "wyse-2" = libx.mkHost { hostname = "wyse-2"; usernames = [ "bcnelson" ]; };
        "wyse-1" = libx.mkHost { hostname = "wyse-1"; usernames = [ "bcnelson" ]; };
        "qilin-1" = libx.mkHost { hostname = "qilin-1"; usernames = [ "bcnelson" "ldporter" ]; desktop = "kde6"; };
        "xray-3" = libx.mkHost { hostname = "xray-3"; usernames = [ "bcnelson" "hlnelson" ]; desktop = "kde6"; };
        "charlie-1" = libx.mkHost { hostname = "charlie-1"; usernames = [ "bcnelson" ]; };
        "golf-4" = libx.mkHost { hostname = "golf-4"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "golf-3" = libx.mkHost { hostname = "golf-3"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "bravo-1" = libx.mkHost { hostname = "bravo-1"; usernames = [ "bcnelson" "brnelson" "hlnelson" ]; desktop = "kde6"; };
        "ryuu-2" = libx.mkHost { hostname = "ryuu-2"; usernames = [ "bcnelson" ]; };
        "berg-1" = libx.mkHost { hostname = "berg-1"; usernames = [ "bcnelson" "dsross" ]; desktop = "kde6"; };
        "sierra-2" = libx.mkHost { hostname = "sierra-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "xray-2" = libx.mkHost { hostname = "xray-2"; usernames = [ "bcnelson" "hlnelson" ]; desktop = "kde6"; };
        "golf-2" = libx.mkHost { hostname = "golf-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "iso_console" = libx.mkHost {
          hostname = "iso_console";
          usernames = [ "nixos" ];
          nixosMods = inputs.nixpkgs-unstable + "/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix";
          channelName = "nixpkgs-unstable-small-patched";
        };
        "iso_desktop" = libx.mkHost { 
          hostname = "iso_desktop";
          usernames = [ "nixos" ];
          nixosMods = inputs.nixpkgs-unstable + "/nixos/modules/installer/cd-dvd/installation-cd-graphical-calamares.nix";
          desktop = "kde6";
        };
        "romeo-2" = libx.mkHost { hostname = "romeo-2"; usernames = [ "bcnelson" ]; };
        "whiskey-1" = libx.mkHost { hostname = "whiskey-1"; usernames = [ "bcnelson" ]; nixosMods = inputs.disko.nixosModules.disko; };
        "vor-2" = libx.mkHost { hostname = "vor-2"; usernames = [ "bcnelson" ]; };
      };

      outputsBuilder = channels: let
        pkgs = channels.nixpkgs-unstable;
      in {
        formatter = inputs.nix-formatter-pack.lib.mkFormatter {
          inherit pkgs;
          config.tools = {
            alejandra.enable = false;
            deadnix.enable = true;
            statix.enable = true;
          };
        };

        # herdr-web is not in pkgs/default.nix because its src is a flake input;
        # add it here too so `nix build .#herdr-web` still works. Same
        # instantiation as overlays/default.nix.
        packages = (import ./pkgs pkgs) // {
          herdr-web = pkgs.callPackage ./pkgs/herdr-web { src = inputs.herdr-web; };
        };

        devShells = let
          pkgsWithOverlays = import inputs.nixpkgs-unstable {
            inherit (pkgs.stdenv.hostPlatform) system;
            config.allowUnfree = true;
            overlays = [
              inputs.agenix-rekey.overlays.default
              inputs.rust-overlay.overlays.default
              self.overlays.modifications
            ];
          };
        in import ./shell.nix {
          inherit inputs;
          outputs = self;
          pkgs = pkgsWithOverlays;
          inherit (pkgs.stdenv.hostPlatform) system;
          inherit (inputs.nixpkgs-unstable) lib;
        };
      };

      overlays = import ./overlays { inherit inputs; };
      nixosModules = import ./modules/nixos;
      homeModules = import ./modules/home-manager;

      # Standalone home-manager for non-NixOS hosts
      homeConfigurations = {
        "bcnelson@redo-3" = libx.mkStandaloneHome {
          hostname = "redo-3";
          username = "bcnelson";
          desktop = "kde6";
        };
      };

      # system-manager for non-NixOS hosts
      systemConfigs = {
        "redo-3" = libx.mkSystemManager {
          hostname = "redo-3";
          usernames = [ "bcnelson" ];
          desktop = "kde6";
        };
      };

      agenix-rekey = inputs.agenix-rekey.configure {
        inherit (self) nixosConfigurations;
        userFlake = self;
      };
    };
}
