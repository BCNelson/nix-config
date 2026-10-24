{ lib
, stdenvNoCC
, fetchurl
, installShellFiles
}:

# Upwind Security's CLI for onboarding hosts, clusters and cloud accounts.
#
# Upstream only ships it through `curl -fsSL https://get.upwind.io/upwindctl.sh
# | sh`, which detects the platform, downloads a single binary from
# releases.upwind.io and runs `upwindctl tool install` to copy itself into the
# home directory. The binary is a statically linked Go executable, so fetching
# it directly is the whole package -- no autoPatchelf, and no self-install into
# a mutable path. Bump `version` to the installer's UPWINDCTL_STABLE_VERSION and
# regenerate hashes with `nix store prefetch-file <url>`.
let
  version = "0.41.1";

  sources = {
    x86_64-linux = {
      arch = "linux-amd64";
      hash = "sha256-d0BtrNgT+EHqdhYx7pElu1VGgtUur1mvi79ZPzNZBJM=";
    };
    aarch64-linux = {
      arch = "linux-arm64";
      hash = "sha256-qm5xBqJsbm6bREKxJrLgEG8FOobAVFRZjWH14frO2zs=";
    };
  };

  source =
    sources.${stdenvNoCC.hostPlatform.system}
      or (throw "upwindctl: unsupported system ${stdenvNoCC.hostPlatform.system}");
in
stdenvNoCC.mkDerivation {
  pname = "upwindctl";
  inherit version;

  src = fetchurl {
    url = "https://releases.upwind.io/upwindctl/v${version}/upwindctl-v${version}-${source.arch}";
    inherit (source) hash;
  };

  nativeBuildInputs = [ installShellFiles ];

  dontUnpack = true;
  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    install -Dm755 $src $out/bin/upwindctl

    runHook postInstall
  '';

  postInstall = lib.optionalString (stdenvNoCC.buildPlatform.canExecute stdenvNoCC.hostPlatform) ''
    installShellCompletion --cmd upwindctl \
      --bash <($out/bin/upwindctl completion bash) \
      --fish <($out/bin/upwindctl completion fish) \
      --zsh <($out/bin/upwindctl completion zsh)
  '';

  meta = {
    description = "Upwind Security CLI for onboarding hosts, clusters and cloud accounts";
    homepage = "https://docs.upwind.io";
    # Per upstream's Homebrew formula (upwindsecurity/homebrew-tap). The source
    # repo it names, upwindsecurity/upwindctl, is private, hence the binary.
    license = lib.licenses.asl20;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = "upwindctl";
    platforms = lib.attrNames sources;
  };
}
