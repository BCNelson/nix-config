{ buildGoModule, lib }:

buildGoModule {
  pname = "codex-config-merge";
  version = "0.1.0";

  src = ./.;

  vendorHash = "sha256-hj1rQJED2llW782lPYYWDD1TgNgHPa0z9nUdj4kWryw=";

  meta = {
    description = "Merge Home Manager Codex base config with Codex runtime trust state";
    homepage = "https://github.com/openai/codex";
    license = lib.licenses.mit;
    maintainers = [ ];
    mainProgram = "codex-config-merge";
  };
}
