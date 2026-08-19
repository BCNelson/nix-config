{ config, pkgs, ... }:
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
  # This used to be a file the user placed here by hand, because standalone
  # home-manager has no agenix. It is now agenix-managed like everywhere else:
  # redo-3's system-manager config declares the secret against the same rekeyFile
  # romeo and sierra use, and decrypts it here at activation. See
  # ../../../../system-manager/redo-3.nix and
  # ../../../../system-manager/_mixins/agenix.nix.
  #
  # Hardcoded rather than read from config.age.secrets: this is a home-manager
  # module and the secret is declared on the system-manager side, which
  # home-manager cannot see. It is agenix's default secretsDir, and the
  # system-manager mixin leaves that default alone.
  secretFile = "/run/agenix/goose-server-secret-key";

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
               echo "Run: systemctl status agenix-install-secrets" >&2
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
