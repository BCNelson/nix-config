{
  buildGoModule,
  lib,
}:
buildGoModule {
  pname = "gamestream-agent";
  version = "0.1.0";

  src = ./.;

  # Dependencies are vendored in-tree (vendor/), so no hash is needed.
  vendorHash = null;

  # Run the Go test suite as part of the build. The pure engine/config/discovery/
  # notify tests are hermetic (they only use unix sockets under TMPDIR), so they
  # pass inside the Nix sandbox. The paho/D-Bus adapters are exercised by the
  # nixos-test.nix VM check instead.
  doCheck = true;

  meta = {
    description = "MQTT<->systemd bridge letting Home Assistant control on-demand Sunshine game-streaming sessions";
    license = lib.licenses.mit;
    maintainers = [ ];
    mainProgram = "gamestream-agent";
  };
}
