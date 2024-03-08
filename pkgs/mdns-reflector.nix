{ stdenv, fetchFromGitHub, cmake }: stdenv.mkDerivation rec {
  pname = "mdns-reflector";
  version = "4b4cd3b196f09b507d9a32c7488491bbd5071ba6";
  src = fetchFromGitHub {
    owner = "vfreex";
    repo = "mdns-reflector";
    rev = "4b4cd3b196f09b507d9a32c7488491bbd5071ba6";
    sha256 = "sha256-HSLBPyAg0Mnv8ksd8BIlOrM8tDVc3VZdgl5u4xUEDTo=";
  };
  buildInputs = [ cmake ];
}
