{
  buildGoModule,
  go_1_27,
  lib,
}:
# Tailscale SSH transport for herdr-core: `hc machine add --via 'hc-tailscale
# ssh user@host'`. See home-manager/bcnelson/_mixins/herdr/hc.nix.
#
# Go 1.27 because tailscale.com (tsnet) needs it; nixpkgs' own tailscale is
# built with the same toolchain.
(buildGoModule.override { go = go_1_27; }) {
  pname = "hc-tailscale";
  version = "0.1.0";

  src = ./.;

  vendorHash = "sha256-t8jPOWOVGmrLuDNVA8onRCryhpOrj4x7d14AksGqto0=";

  # Strip symbols: tsnet pulls in most of tailscaled, so this halves the size.
  ldflags = [ "-s" "-w" ];

  meta = {
    description = "Tailscale (tsnet) SSH transport for herdr-core machines, one tailnet login per slot";
    license = lib.licenses.mit;
    maintainers = [ ];
    mainProgram = "hc-tailscale";
  };
}
