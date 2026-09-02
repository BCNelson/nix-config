{ lib
, stdenv
, fetchzip
, autoPatchelfHook
}:

# CockroachDB CLI/server binary, pinned to current stable.
#
# nixpkgs' `cockroachdb` is stuck on 23.1.14 (Jan 2024) -- too old to talk to
# the CockroachDB Cloud cluster in ../home-manager/bcnelson/_mixins/mcp/
# redo-production-crdb.nix without version-skew warnings -- and it wraps the
# binary in a buildFHSEnv chroot. Upstream only links against glibc (libstdc++
# is static in the bundled geos), so plain autoPatchelf is enough and drops the
# bubblewrap layer.
#
# Upstream builds no longer work from source; see
# https://github.com/NixOS/nixpkgs/pull/152626. Regenerate hashes with
# `nix flake prefetch <url>`.
let
  version = "26.2.6";

  sources = {
    x86_64-linux = {
      arch = "linux-amd64";
      hash = "sha256-dh1561vbEZCOSTDUUKIjLN9Kf3jfYs+EmZTKN4HDmGM=";
    };
    aarch64-linux = {
      arch = "linux-arm64";
      hash = "sha256-vvz5BtvOlz2szKOCcPMeApsnXBpm0X/zghymGO/Ajtw=";
    };
  };

  source =
    sources.${stdenv.hostPlatform.system}
      or (throw "cockroachdb: unsupported system ${stdenv.hostPlatform.system}");
in
stdenv.mkDerivation {
  pname = "cockroachdb";
  inherit version;

  src = fetchzip {
    url = "https://binaries.cockroachdb.com/cockroach-v${version}.${source.arch}.tgz";
    inherit (source) hash;
  };

  nativeBuildInputs = [ autoPatchelfHook ];

  dontConfigure = true;
  dontBuild = true;

  # $out/lib is one of the directories cockroach probes for the geos libraries
  # it dlopens for spatial types (it walks up from the binary's own path), so
  # this layout needs no --spatial-libs flag.
  installPhase = ''
    runHook preInstall

    install -Dm755 cockroach $out/bin/cockroach
    install -Dm644 -t $out/lib lib/libgeos.so lib/libgeos_c.so
    install -Dm644 -t $out/share/doc/cockroachdb LICENSE THIRD-PARTY-NOTICES.txt

    runHook postInstall
  '';

  meta = {
    description = "Scalable, survivable, strongly-consistent SQL database";
    homepage = "https://www.cockroachlabs.com";
    license = with lib.licenses; [
      bsl11
      mit
      cockroachdb-community-license
    ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = "cockroach";
    platforms = lib.attrNames sources;
  };
}
