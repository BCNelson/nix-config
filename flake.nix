{
  description = "Bcnleson's NixOS configuration";

  inputs = {
    # Nixpkgs
    nixpkgs24-05.url = "github:nixos/nixpkgs/nixos-24.05";
    nixpkgs24-11.url = "github:nixos/nixpkgs/nixos-24.11";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";

    # Home manager
    home-manager24-05.url = "github:nix-community/home-manager/release-24.05";
    home-manager24-05.inputs.nixpkgs.follows = "nixpkgs24-05";

    home-manager24-11.url = "github:nix-community/home-manager/release-24.11";
    home-manager24-11.inputs.nixpkgs.follows = "nixpkgs24-11";

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
    nix-formatter-pack.inputs.nixpkgs.follows = "nixpkgs24-05";

    disko.url = "github:nix-community/disko";
    disko.inputs.nixpkgs.follows = "nixpkgs24-05";

    nixos-hardware.url = "github:nixos/nixos-hardware";

    # Add the Nix User Repository (NUR)
    nur.url = "github:nix-community/NUR";

    nixos-generators = {
      url = "github:nix-community/nixos-generators";
      inputs.nixpkgs.follows = "nixpkgs24-05";
    };

    catppuccin.url = "github:catppuccin/nix";

    nix-darwin.url = "github:LnL7/nix-darwin/master";
    nix-darwin.inputs.nixpkgs.follows = "nixpkgs-unstable";

    nixarr.url = "github:rasmus-kirk/nixarr/dev";
  };

  outputs = { self, nix-formatter-pack, nixpkgs-unstable, disko, ... }@inputs:
    let
      inherit (self) outputs;
      # This value determines the Home Manager release that your configuration is
      # compatible with. This helps avoid breakage when a new Home Manager release
      # introduces backwards incompatible changes.
      #
      # You should not change this value, even if you update Home Manager. If you do
      # want to update the value, then make sure to first check the Home Manager
      # release notes.
      stateVersion = "23.05";
      libx = import ./lib { inherit inputs outputs stateVersion; };
    in
    {
      # Your custom packages and modifications, exported as overlays
      overlays = import ./overlays { inherit inputs; };
      # Reusable nixos modules you might want to export
      # These are usually stuff you would upstream into nixpkgs
      nixosModules = import ./modules/nixos;
      # Reusable home-manager modules you might want to export
      # These are usually stuff you would upstream into home-manager
      homeManagerModules = import ./modules/home-manager;

      # NixOS configuration entrypoint
      # Available through 'nixos-rebuild --flake .#your-hostname'
      nixosConfigurations = {
        "golf-3" = libx.mkHost { hostname = "golf-3"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "bravo-1" = libx.mkHost { hostname = "bravo-1"; usernames = [ "bcnelson" "brnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "ryuu-2" = libx.mkHost { hostname = "ryuu-2"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        "berg-1" = libx.mkHost { hostname = "berg-1"; usernames = [ "bcnelson" "dsross" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "sierra-2" = libx.mkHost { hostname = "sierra-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "xray-2" = libx.mkHost { hostname = "xray-2"; usernames = [ "bcnelson" "hlnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "golf-2" = libx.mkHost { hostname = "golf-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "iso_console" = libx.mkHost { hostname = "iso_console"; usernames = [ "nixos" ]; nixosMods = inputs.nixpkgs24-11 + "/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"; inherit libx; version = "unstable"; };
        "iso_desktop" = libx.mkHost { hostname = "iso_desktop"; usernames = [ "nixos" ]; nixosMods = inputs.nixpkgs24-11 + "/nixos/modules/installer/cd-dvd/installation-cd-graphical-calamares.nix"; desktop = "kde6"; inherit libx; version = "unstable"; };
        "vm_test" = libx.mkHost { hostname = "vm_test"; usernames = [ "bcnelson" "brnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "romeo-2" = libx.mkHost { hostname = "romeo-2"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        "whiskey-1" = libx.mkHost { hostname = "whiskey-1"; usernames = [ "bcnelson" ]; inherit libx; nixosMods = disko.nixosModules.disko; version = "unstable"; };
        "vor-2" = libx.mkHost { hostname = "vor-2"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        # "delta-1" = libx.mkHost { hostname = "delta-1"; usernames = [ "bcnelson" ]; inherit libx; version = "stable"; };
        # INSERT_HOST_CONFIG
        "redo-1" = libx.mkHost { hostname = "redo-1"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
      };

      formatter = libx.forAllSystems (system:
        nix-formatter-pack.lib.mkFormatter {
          pkgs = nixpkgs-unstable.legacyPackages.${system};
          config.tools = {
            alejandra.enable = false;
            deadnix.enable = true;
            statix.enable = true;
          };
        }
      );

      agenix-rekey = inputs.agenix-rekey.configure {
        inherit (self) nixosConfigurations;
        userFlake = self;
      };

      # Your custom packages
      # Accessible through 'nix build', 'nix shell', etc
      packages = libx.forAllSystems (system: import ./pkgs nixpkgs-unstable.legacyPackages.${system});

      # Devshell for bootstrapping
      # Acessible through 'nix develop' or 'nix-shell' (legacy)
      devShells = libx.forAllSystems (system:
        let pkgs = import nixpkgs-unstable { inherit system; config.allowUnfree = true; overlays = [ inputs.agenix-rekey.overlays.default inputs.rust-overlay.overlays.default ]; };
        in import ./shell.nix { inherit inputs outputs pkgs system; inherit (nixpkgs-unstable) lib; }
      );
    };
}
