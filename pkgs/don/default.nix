{
  lib,
  rustPlatform,
  fetchFromGitHub,
  cacert,
  callPackage,
  git,
  installShellFiles,
  pkg-config,
  runCommand,
  writeShellScriptBin,
  zig_0_15,
}:
let
  # don depends on libghostty-vt, whose -sys crate builds ghostty's terminal
  # library from source with zig. Its build script clones a pinned ghostty
  # revision unless GHOSTTY_SOURCE_DIR points at a checkout, which the sandbox
  # can't do. Keep this rev in sync with GHOSTTY_COMMIT in
  # libghostty-vt-sys/build.rs whenever the libghostty-vt dep moves.
  ghosttySrc = fetchFromGitHub {
    owner = "ghostty-org";
    repo = "ghostty";
    rev = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
    hash = "sha256-1Zz65SCk3rkJ9+Q0MmyNOTNiDSLBRIHRd3IvFM4iNXw=";
  };

  # Same story for zig's own package fetches: ghostty ships a zon2nix-generated
  # expression, so materialise the package set here and hand it to
  # `zig build --system` via GHOSTTY_ZIG_SYSTEM_DIR.
  ghosttyZigDeps = callPackage "${ghosttySrc}/build.zig.zon.nix" {
    name = "ghostty-zig-packages";
    # zig's build runner computes relative paths lexically, so symlinked deps
    # resolve to the wrong depth. Copy instead of linking, as ghostty's own
    # nix/libghostty-vt.nix does.
    linkFarm =
      name: entries:
      runCommand name { } ''
        mkdir -p $out
        ${lib.concatMapStringsSep "\n" (e: ''cp -rL ${e.path} $out/${e.name}'') entries}
      '';
  };

  # The build script hardcodes its `zig build` arguments, so there is nowhere to
  # pass -Dcpu=baseline. Without it zig targets the builder's CPU and the result
  # is not portable across hosts sharing the binary cache.
  zigBaseline = writeShellScriptBin "zig" ''
    if [ "''${1-}" = "build" ]; then
      shift
      exec ${lib.getExe' zig_0_15 "zig"} build -Dcpu=baseline "$@"
    fi
    exec ${lib.getExe' zig_0_15 "zig"} "$@"
  '';
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "don";
  version = "0.8.0";

  src = fetchFromGitHub {
    owner = "pjtatlow";
    repo = "don";
    tag = "v${finalAttrs.version}";
    hash = "sha256-Ome13D7d6681EW+9URaWZ9DukCjCzBSy7hgBKhSQPp0=";
  };

  cargoHash = "sha256-28XDv8TCo/twcnZTWvbMvE3GyE1v3KVszQKlR3fKQDw=";

  nativeBuildInputs = [
    git
    installShellFiles
    pkg-config
    zigBaseline
  ];

  env.GHOSTTY_ZIG_SYSTEM_DIR = ghosttyZigDeps;

  # zig wants a writable source tree and a HOME; exported in preConfigure so the
  # build, check and install phases all see them.
  preConfigure = ''
    export HOME="$TMPDIR"
    cp -r --no-preserve=mode,ownership ${ghosttySrc} "$NIX_BUILD_TOP/ghostty-src"
    export GHOSTTY_SOURCE_DIR="$NIX_BUILD_TOP/ghostty-src"
  '';

  # don is a bin-only crate, so this runs the unit tests in the binary and skips
  # everything under tests/, which is 13 integration suites that drive real
  # infrastructure: preset_test shells out to cargo/go toolchains, docker_test
  # needs a daemon, download_test wants reqwest's native CA roots. None of that
  # exists in the sandbox.
  cargoTestFlags = [ "--bins" ];

  # download_test also needs a CA bundle before reqwest's builder will even
  # construct a client ("No CA certificates were loaded from the system"),
  # kept here so re-enabling the integration suites is one flag away.
  nativeCheckInputs = [ cacert ];
  preCheck = ''
    export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
  '';

  # `don completions <shell>` emits clap_complete's dynamic script, which shells
  # out to the binary for service/task/profile names from don.toml.
  postInstall = ''
    installShellCompletion --cmd don \
      --bash <($out/bin/don completions bash) \
      --fish <($out/bin/don completions fish) \
      --zsh <($out/bin/don completions zsh)
  '';

  meta = {
    description = "Boss of your dev environment";
    homepage = "https://github.com/pjtatlow/don";
    license = lib.licenses.mit;
    mainProgram = "don";
    platforms = lib.platforms.unix;
  };
})
