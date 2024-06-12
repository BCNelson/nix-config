{ pkgs, ... }:
let
  goVersion = 21; # Change this to update the whole stack
  # This can be moved to a separate file if we like
  protoc-gen-go-grpc-mock = pkgs.buildGoModule {
    pname = "protoc-gen-go-grpc-mock";
    version = "1.2.0";

    src = pkgs.fetchFromGitHub {
      owner = "sorcererxw";
      repo = "protoc-gen-go-grpc-mock";
      rev = "v1.2.0";
      hash = "sha256-IBjSBtv1fn9KOwrFTJk4zUVgil6iTCl4NyuBUQIV09Q=";
    };

    vendorHash = "sha256-qcORCUEgCctJ0gDCJeo6l6de2NaRrzwFcjAfUeL5GDU=";
  };
in
{

  nixpkgs.overlays = [ (_final: prev: { go = prev."go_1_${toString goVersion}"; }) ];
  home.packages = with pkgs; [
    # go 1.21 (specified by overlay)
    go

    # goimports, godoc, etc.
    gotools
    gopls

    # https://github.com/golangci/golangci-lint
    golangci-lint

    buf
    protoc-gen-go-grpc-mock

    go-mockery
    go-migrate
    dapr-cli
    gnumake

    postman
    nodejs_20
    awscli2
  ];
}
