{ desktop, lib, ... }:
let
  atLeast2405 = lib.versionAtLeast lib.trivial.release "24.05";
in
{
  imports = lib.optional (builtins.pathExists ./${desktop}.nix) ./${desktop}.nix;

  services = {
    pipewire = {
      enable = true;
      alsa.enable = true;
      alsa.support32Bit = true;
      pulse.enable = true;
      jack.enable = true;
      wireplumber.enable = true;
    };
    xserver = {
      xkb = {
        layout = "us";
        variant = "";
      };
    };

    # Enable the X11 windowing system.
    xserver.enable = true; #TODO: See if this is needed

    # Enable CUPS to print documents.
    printing.enable = true;
  } // (if atLeast2405 then {
    displayManager.sddm = {
      enable = true;
      autoNumlock = true;
    };
  } else {
    xserver.displayManager.sddm = {
      enable = true;
      autoNumlock = true;
    };
  });

  # Enable sound with pipewire.
  # sound.enable = true; // Removed https://github.com/NixOS/nixpkgs/issues/319809
  hardware.pulseaudio.enable = lib.mkForce false;

  security.polkit.enable = true;

  programs.ssh = {
    startAgent = true;
  };
}
