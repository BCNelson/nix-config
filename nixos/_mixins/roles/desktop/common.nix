_:
{
  # Enable the X11 windowing system.
  services.xserver.enable = true; #TODO: See if this is needed

  services.xserver.displayManager.sddm = {
    enable = true;
    autoNumlock = true;
  };

  services.flatpak.enable = true;
  services.xserver.displayManager.defaultSession = "plasmawayland";

  # Enable CUPS to print documents.
  services.printing.enable = true;

  # Enable sound with pipewire.
  sound.enable = true;
  hardware.pulseaudio.enable = false; #TODO: See if this is needed
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = true;
    pulse.enable = true;
    jack.enable = true;
  };

  system.activationScripts = {
    flathub = ''
      /run/current-system/sw/bin/flatpak remote-add --system --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
    '';
  };

  # Configure keymap in X11
  services.xserver = {
    layout = "us";
    xkbVariant = "";
  };
}
