{ config, lib, pkgs, ... }:
let
  # romeo's ACP endpoint, same URL sierra uses. goose.h.b.nel.family is an
  # explicit CNAME to romeo.b.nel.family (A 100.76.49.168, romeo's tailnet
  # address); an exact name beats the *.h.b.nel.family wildcard, so a roaming
  # host resolves it to the tailnet and reaches nginx with a source address the
  # vhost's allow 100.64.0.0/10 rule accepts. On the LAN romeo's unbound
  # answers the same name with 192.168.3.7 instead. Either way the URL is
  # identical, so this host and sierra no longer need to differ.
  # See nixos/romeo/services/goose.nix.
  backendUrl = "https://goose.h.b.nel.family";

  # Same contract as sierra: GOOSE_EXTERNAL_BACKEND makes the Electron app
  # attach to an existing goosed instead of spawning its own, and the app
  # refuses to start without GOOSE_SERVER__SECRET_KEY when it is set.
  #
  # redo-3 is a non-NixOS host managed by standalone home-manager, so there is
  # no agenix here to decrypt romeo's key into /run/agenix -- agenix-rekey only
  # covers nixosConfigurations. The key is therefore read at launch from a file
  # the user places once, out of band:
  #
  #   install -Dm600 /dev/null ~/.config/goose/server-secret-key
  #   # paste the value from romeo:/run/agenix/goose-server-secret-key
  #
  # Reading it at launch (rather than --set) also keeps it out of the
  # world-readable store, which is why sierra does the same.
  secretFile = "${config.xdg.configHome}/goose/server-secret-key";

  goose-desktop-romeo = pkgs.runCommand "goose-desktop-romeo"
    {
      nativeBuildInputs = [ pkgs.makeWrapper ];
      meta.mainProgram = "goose-desktop";
    } ''
    mkdir -p $out/bin $out/share/applications

    makeWrapper ${pkgs.goose-desktop}/bin/goose-desktop $out/bin/goose-desktop \
      --set GOOSE_EXTERNAL_BACKEND 1 \
      --set GOOSE_EXTERNAL_BACKEND_URL "${backendUrl}" \
      --run 'if [ ! -r "${secretFile}" ]; then
               echo "goose-desktop: missing ${secretFile}" >&2
               echo "Copy romeo:/run/agenix/goose-server-secret-key into it (mode 0600)." >&2
               exit 1
             fi' \
      --run 'export GOOSE_SERVER__SECRET_KEY="$(cat "${secretFile}")"'

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
  # nixGL goes on the outside: it re-wraps every executable in the package and
  # rewrites the .desktop Exec= to match, so wrapping the env wrapper (rather
  # than pkgs.goose-desktop directly) keeps a single launcher that sets the GL
  # loader paths first and the GOOSE_* variables second.
  home.packages = [ (config.lib.nixGL.wrap goose-desktop-romeo) ];
}
