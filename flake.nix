{
  description = "Bcnleson's NixOS configuration";

  inputs = {
    # Nixpkgs
    nixpkgs23-11.url = "github:nixos/nixpkgs/nixos-23.11";
    nixpkgs24-05.url = "github:nixos/nixpkgs/nixos-24.05";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";

    # Home manager
    home-manager23-11.url = "github:nix-community/home-manager/release-23.11";
    home-manager23-11.inputs.nixpkgs.follows = "nixpkgs23-11";

    home-manager24-05.url = "github:nix-community/home-manager/release-24.05";
    home-manager24-05.inputs.nixpkgs.follows = "nixpkgs24-05";

    home-manager-unstable.url = "github:nix-community/home-manager/master";
    home-manager-unstable.inputs.nixpkgs.follows = "nixpkgs-unstable";

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

    nix-darwin.url = "github:LnL7/nix-darwin/master";
    nix-darwin.inputs.nixpkgs.follows = "nixpkgs-unstable";
  };

  outputs = { self, nix-formatter-pack, nixpkgs-unstable, home-manager-unstable, disko, ... }@inputs:
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
    rec {
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
        "sierra-2" = libx.mkHost { hostname = "sierra-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "xray-2" = libx.mkHost { hostname = "xray-2"; usernames = [ "bcnelson" "hlnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "golf-2" = libx.mkHost { hostname = "golf-2"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
        "iso_console" = libx.mkHost { hostname = "iso_console"; usernames = [ "nixos" ]; nixosMods = inputs.nixpkgs24-05 + "/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"; inherit libx; };
        "iso_desktop" = libx.mkHost { hostname = "iso_desktop"; usernames = [ "nixos" ]; nixosMods = inputs.nixpkgs24-05 + "/nixos/modules/installer/cd-dvd/installation-cd-graphical-calamares.nix"; desktop = "kde"; inherit libx; };
        # "vm_test" = libx.mkHost { hostname = "vm_test"; username = "bcnelson"; desktop = "kde"; };
        "romeo-2" = libx.mkHost { hostname = "romeo-2"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        "whiskey-1" = libx.mkHost { hostname = "whiskey-1"; usernames = [ "bcnelson" ]; inherit libx; nixosMods = disko.nixosModules.disko; version = "unstable"; };
        "vor-2" = libx.mkHost { hostname = "vor-2"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        "kilo-1" = libx.mkHost { hostname = "kilo-1"; usernames = [ "bcnelson" ]; inherit libx; version = "unstable"; };
        "delta-1" = libx.mkHost { hostname = "delta-1"; usernames = [ "bcnelson" ]; inherit libx; version = "stable"; };
        # INSERT_HOST_CONFIG
        "redo-1" = libx.mkHost { hostname = "redo-1"; usernames = [ "bcnelson" ]; desktop = "kde6"; inherit libx; version = "unstable"; };
      };

      # Standalone home-manager configuration entrypoint
      # Available through 'home-manager --flake .#your-username@your-hostname'
      homeConfigurations = {
        "bnelson@GX00087" = libx.mkHome { hostname = "GX00087"; usernames = [ "bnelson" ]; platform = "aarch64-darwin"; home-manager = home-manager-unstable; pkgs = nixpkgs-unstable; };
      };

      darwinConfigurations = {
        "GX00087" = libx.mkDarwin { hostname = "GX00087"; usernames = [ "bnelson" ]; platform = "aarch64-darwin"; version = "unstable"; };
      };

      formatter = libx.forAllSystems (system:
        nix-formatter-pack.lib.mkFormatter {
          pkgs = nixpkgs-unstable.legacyPackages.${system};
          config.tools = {
            alejandra.enable = false;
            deadnix.enable = true;
            nixpkgs-fmt.enable = true;
            statix.enable = true;
          };
        }
      );

      # Your custom packages
      # Accessible through 'nix build', 'nix shell', etc
      packages = libx.forAllSystems (system: import ./pkgs nixpkgs-unstable.legacyPackages.${system});

      # Devshell for bootstrapping
      # Acessible through 'nix develop' or 'nix-shell' (legacy)
      devShells = libx.forAllSystems (system:
        let pkgs = import nixpkgs-unstable { inherit system; config.allowUnfree = true; };
        in import ./shell.nix { inherit inputs outputs pkgs system; inherit (nixpkgs-unstable) lib; }
      );
    };
}
