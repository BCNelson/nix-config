{ config, pkgs, ... }:
let
  # romeo's ACP endpoint. nginx there restricts access to the tailnet
  # (100.64.0.0/10) and the LAN, so this resolves over Tailscale when away.
  backendUrl = "https://goose.h.b.nel.family";

  # Goose Desktop normally spawns its own local goosed. Setting
  # GOOSE_EXTERNAL_BACKEND makes it attach to an existing server instead --
  # from the app's own bundle:
  #
  #   if (!process.env.GOOSE_EXTERNAL_BACKEND) return null;
  #   const t = process.env.GOOSE_EXTERNAL_BACKEND_URL?.trim();
  #   return t || `http://127.0.0.1:${process.env.GOOSE_PORT || "3000"}`;
  #   ... throws "GOOSE_SERVER__SECRET_KEY must be set when using GOOSE_EXTERNAL_BACKEND"
  #
  # The consequence worth remembering: the agent executes on romeo, inside the
  # /var/lib/goose sandbox, not on sierra. This UI edits romeo's files.
  #
  # The secret is read at launch rather than baked in with --set, because
  # everything in /nix/store is world-readable.
  goose-desktop-romeo = pkgs.runCommand "goose-desktop-romeo"
    {
      nativeBuildInputs = [ pkgs.makeWrapper ];
      meta.mainProgram = "goose-desktop";
    } ''
    mkdir -p $out/bin $out/share/applications

    makeWrapper ${pkgs.goose-desktop}/bin/goose-desktop $out/bin/goose-desktop \
      --set GOOSE_EXTERNAL_BACKEND 1 \
      --set GOOSE_EXTERNAL_BACKEND_URL "${backendUrl}" \
      --run 'export GOOSE_SERVER__SECRET_KEY="$(cat ${config.age.secrets.goose-server-secret-key.path})"'

    # Point the launcher at the wrapper rather than the unwrapped binary, which
    # would start with no backend configured and fail on the missing secret.
    substitute ${pkgs.goose-desktop}/share/applications/goose.desktop \
      $out/share/applications/goose.desktop \
      --replace-quiet "${pkgs.goose-desktop}/bin/goose-desktop" "$out/bin/goose-desktop"

    if [ -d ${pkgs.goose-desktop}/share/icons ]; then
      cp -r ${pkgs.goose-desktop}/share/icons $out/share/
    fi
    if [ -d ${pkgs.goose-desktop}/share/pixmaps ]; then
      cp -r ${pkgs.goose-desktop}/share/pixmaps $out/share/
    fi
  '';
in
{
  # Same source file romeo uses, so both hosts decrypt the identical value --
  # agenix-rekey re-encrypts it per host. owner is set because the GUI runs as
  # bcnelson and has to read it at launch.
  age.secrets.goose-server-secret-key = {
    rekeyFile = ../romeo/services/secrets/goose_server_secret_key.age;
    owner = "bcnelson";
    mode = "0400";
  };

  environment.systemPackages = [ goose-desktop-romeo ];
}
