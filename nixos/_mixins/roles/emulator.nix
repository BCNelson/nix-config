_:

{
  services.udev = {
    enable = true;
    # Enable Stream Deck support
    extraRules = ''
      SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTRS{idVendor}=="0079", ATTRS{idProduct}=="1843", MODE="0666"
    '';
  };
}