{ pkgs, ... }:
{
  # UniFi Network controller. State lives in /var/lib/unifi (fixed by the
  # module); the controller's own autobackups land in data/backup there.
  services.unifi = {
    enable = true;
    # vor's stateVersion predates UniFi 8, so the module refuses to pick these
    # for us (it can't know whether an old MongoDB 5.0 database needs a
    # stepwise upgrade). This is a fresh install, so 7.0 directly is fine.
    # 10.6 started shipping libubnt_webrtc_jni.so, which links libstdc++ and
    # libgcc_s. nixpkgs builds it with stdenvNoCC and only systemd in
    # buildInputs, so autoPatchelf can't satisfy them and the build fails.
    # Drop this once nixpkgs adds the C++ runtime itself.
    unifiPackage = pkgs.unifi.overrideAttrs (old: {
      buildInputs = old.buildInputs ++ [ pkgs.stdenv.cc.cc.lib ];
    });
    mongodbPackage = pkgs.mongodb-7_0;
    # Device discovery, STUN and the inform port (8080) that APs/switches use
    # to adopt; without these, adoption and firmware upgrades fail.
    openFirewall = true;
  };

  # Web UI (self-signed HTTPS). openFirewall deliberately leaves it closed.
  networking.firewall.allowedTCPPorts = [ 8443 ];
}
