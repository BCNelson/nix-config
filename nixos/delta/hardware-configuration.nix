{ inputs, ... }:

{
  imports =
    [
      inputs.nixos-hardware.nixosModules.raspberry-pi-4
    ];
  nixpkgs.hostPlatform.system = "aarch64-linux";
  nixpkgs.overlays = [
    (_final: super: {
      makeModulesClosure = x:
        super.makeModulesClosure (x // { allowMissing = true; });
    })
  ];
}
