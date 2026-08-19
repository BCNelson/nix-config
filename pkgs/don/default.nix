{
  lib,
  rustPlatform,
  fetchFromGitHub,
  cacert,
  installShellFiles,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "don";
  version = "0.7.2";

  src = fetchFromGitHub {
    owner = "pjtatlow";
    repo = "don";
    tag = "v${finalAttrs.version}";
    hash = "sha256-t5dL/28CTX5aFD/Yvn+NOI6G78XFCkWDXnxU0duTW8s=";
  };

  cargoHash = "sha256-ZcZ0bT0dcRKGtgnHDDtyrf5Nrw9TjGoqSqp7sXBviYg=";

  nativeBuildInputs = [ installShellFiles ];

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
