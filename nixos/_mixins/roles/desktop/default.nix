{ desktop, lib, ... }: {
  imports = lib.optional (builtins.pathExists ./${desktop}.nix) ./${desktop}.nix;
  # Enable the X11 windowing system.
  services.xserver.enable = true; #TODO: See if this is needed

  services.displayManager.sddm = {
    enable = true;
    autoNumlock = true;
  };

  # Enable CUPS to print documents.
  services.printing.enable = true;

  # Enable sound with pipewire.
  sound.enable = true;
  hardware.pulseaudio.enable = lib.mkForce false;
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = true;
    pulse.enable = true;
    jack.enable = true;
    wireplumber.enable = true;
  };

  # Configure keymap in X11
  services.xserver = {
    xkb = {
      layout = "us";
      variant = "";
    };
  };

  programs.ssh = {
    startAgent = true;
  };
}
