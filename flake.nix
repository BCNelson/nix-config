{
  description = "Bcnleson's NixOS configuration";

  inputs = {
    # Flake utils
    flake-utils-plus.url = "github:gytis-ivaskevicius/flake-utils-plus";
    
    # Nixpkgs
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";
    nixpkgs-unstable-small.url = "github:nixos/nixpkgs/nixos-unstable-small";

    # Home manager - always use unstable
    home-manager-unstable.url = "github:nix-community/home-manager/master";
    home-manager-unstable.inputs.nixpkgs.follows = "nixpkgs-unstable";

    agenix.url = "github:ryantm/agenix";
    agenix-rekey.url = "github:oddlama/agenix-rekey";
    agenix-rekey.inputs.nixpkgs.follows = "nixpkgs-unstable";
    agenix-template.url = "github:jhillyerd/agenix-template/1.0.0";

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

    catppuccin.url = "github:catppuccin/nix";

    nix-darwin.url = "github:LnL7/nix-darwin/master";
    nix-darwin.inputs.nixpkgs.follows = "nixpkgs-unstable";

    nixarr.url = "github:rasmus-kirk/nixarr/dev";
  };

  outputs = inputs@{ self, flake-utils-plus, ... }:
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
          inputs.catppuccin.nixosModules.catppuccin
          inputs.agenix.nixosModules.default
          inputs.agenix-rekey.nixosModules.default
          inputs.agenix-template.nixosModules.default
        ] ++ (builtins.attrValues (import ./modules/nixos));
      };
      
      hosts = let
        libx = import ./lib { inherit inputs; stateVersion = "23.05"; outputs = self; };
      in {
        # INSERT_NEW_HOST_CONFIG_HERE
        "charlie-1" = libx.mkHost { hostname = "charlie-1"; usernames = [ "bcnelson" ]; };
        "golf-4" = libx.mkHost { hostname = "golf-4"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "redo-2" = libx.mkHost { hostname = "redo-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "golf-3" = libx.mkHost { hostname = "golf-3"; usernames = [ "bcnelson" ]; desktop = "kde6"; };
        "bravo-1" = libx.mkHost { hostname = "bravo-1"; usernames = [ "bcnelson" "brnelson" ]; desktop = "kde6"; };
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

        packages = import ./pkgs pkgs;

        devShells = let 
          pkgsWithOverlays = import inputs.nixpkgs-unstable { 
            inherit (pkgs) system; 
            config.allowUnfree = true; 
            overlays = [ 
              inputs.agenix-rekey.overlays.default 
              inputs.rust-overlay.overlays.default 
            ]; 
          };
        in import ./shell.nix { 
          inherit inputs; 
          outputs = self; 
          pkgs = pkgsWithOverlays; 
          inherit (pkgs) system; 
          inherit (inputs.nixpkgs-unstable) lib; 
        };
      };

      overlays = import ./overlays { inherit inputs; };
      nixosModules = import ./modules/nixos;
      homeManagerModules = import ./modules/home-manager;

      agenix-rekey = inputs.agenix-rekey.configure {
        inherit (self) nixosConfigurations;
        userFlake = self;
      };
    };
}
