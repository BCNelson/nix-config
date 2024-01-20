{ pkgs, ... }:
{
  home.packages = with pkgs; [
    # (retroarch.override {
    #   cores = with libretro; [
    #     pcsx2
    #     parallel-n64
    #     dolphin
    #     desmume
    #   ];
    # })
    # pcsx2
    dolphin-emu
  ];
}
