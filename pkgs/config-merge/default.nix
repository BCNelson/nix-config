{ buildGoModule, lib }:

buildGoModule {
  pname = "config-merge";
  version = "0.2.0";

  src = ./.;

  vendorHash = "sha256-hj1rQJED2llW782lPYYWDD1TgNgHPa0z9nUdj4kWryw=";

  meta = {
    description = "Merge a read-only declarative base config with the runtime state an application writes for itself";
    license = lib.licenses.mit;
    maintainers = [ ];
    mainProgram = "config-merge";
  };
}
