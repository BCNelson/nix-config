{
  lib,
  stdenvNoCC,
  fetchurl,
  installShellFiles,
  testers,
}:
# goose is not in nixpkgs. Upstream moved from block/goose to aaif-goose/goose
# and publishes a per-target release matrix; we take the
# x86_64-unknown-linux-musl tarball because it is a static-pie binary with no
# DT_NEEDED entries at all, so it needs neither autoPatchelfHook nor an FHS
# wrapper. The -gnu tarball would need patchelf'ing, and building from the
# 314 MB source tarball means a full Rust workspace compile on every bump --
# unacceptable on romeo, which rebuilds itself hourly via auto-update.
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "goose";
  version = "1.45.0";

  src = fetchurl {
    url = "https://github.com/aaif-goose/goose/releases/download/v${finalAttrs.version}/goose-x86_64-unknown-linux-musl.tar.bz2";
    hash = "sha256-NwI6f6mrAyrD0je8OB4ebzBxuWY1M4HDG+bejObcuyk=";
  };

  # The tarball is a bare ./goose with no version directory to strip.
  sourceRoot = ".";

  nativeBuildInputs = [installShellFiles];

  dontConfigure = true;
  dontBuild = true;

  # Static-pie, already stripped of nothing we can usefully remove: the binary
  # is ~142 MB because it embeds the full extension/tokenizer payload, and
  # stripping it does not meaningfully shrink that while it does discard the
  # build metadata `goose info` reports.
  dontStrip = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 goose $out/bin/goose
    runHook postInstall
  '';

  # Safe to execute the binary here: it is x86_64 and the package is
  # x86_64-linux only, so there is no cross-compilation case to guard.
  postInstall = ''
    installShellCompletion --cmd goose \
      --bash <($out/bin/goose completion bash) \
      --zsh <($out/bin/goose completion zsh) \
      --fish <($out/bin/goose completion fish)
  '';

  passthru.tests.version = testers.testVersion {
    package = finalAttrs.finalPackage;
    # `goose --version` prints the bare version with no program name prefix.
    inherit (finalAttrs) version;
  };

  meta = {
    description = "Open source, extensible AI agent that installs, executes, edits, and tests with any LLM";
    homepage = "https://goose-docs.ai/";
    license = lib.licenses.asl20;
    mainProgram = "goose";
    platforms = ["x86_64-linux"];
    sourceProvenance = [lib.sourceTypes.binaryNativeCode];
  };
})
