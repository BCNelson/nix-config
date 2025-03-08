{ desktop, lib, pkgs, ... }: {
  imports = lib.optional (builtins.pathExists ./${desktop}.nix) ./${desktop}.nix;

  services = {
    pipewire = {
      enable = true;
      alsa.enable = true;
      alsa.support32Bit = true;
      pulse.enable = true;
      jack.enable = true;
      wireplumber = {
        enable = true;
        extraConfig.bluetoothEnhancements = {
          "monitor.bluez.properties" = {
            "bluez5.enable-sbc-xq" = true;
            "bluez5.enable-msbc" = true;
            "bluez5.enable-hw-volume" = true;
            "bluez5.roles" = [ "a2dp_sink" "a2dp_source" "bap_sink" "bap_source" "hsp_hs" "hsp_ag" "hfp_hf" "hfp_ag" ];
          };
        };
      };
    };
    pulseaudio.enable = lib.mkForce false;
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
    # Enable the Avahi mDNS/DNS-SD daemon. Printer discovery. and other things
    avahi = {
      enable = true;
      nssmdns4 = true;
      openFirewall = true;
    };
  };

  # Enable sound with pipewire.
  # sound.enable = true; // Removed https://github.com/NixOS/nixpkgs/issues/319809

  security.polkit.enable = true;

  programs.ssh = {
    startAgent = true;
  };

  programs.gnupg.agent = {
    enable = true;
    pinentryPackage = pkgs.pinentry-qt;
  };
}
