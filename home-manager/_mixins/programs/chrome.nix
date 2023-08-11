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
    ];
  };
}
