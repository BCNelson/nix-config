_:

{
  programs.chromium = {
    enable = true;
    extensions = [
      { id = "nngceckbapebfimnlniiiahkandclblb"; } # bitwarden
      { id = "eimadpbcbfnmbkopoojfekhnkhdbieeh"; } # darkreader
      { id = "gbmgphmejlcoihgedabhgjdkcahacjlj"; } # wallabagger
    ];
    commandLineArgs = [
      "--enable-ozone"
      "--ozone-platform=wayland"
      "--enable-features=UseOzonePlatform,VaapiVideoDecoder"
      "--ignore-gpu-blocklist"
      "--enable-gpu-rasterization"
      "--enable-accelerated-video-decode"
      "--enable-zero-copy"
    ];
  };
}
