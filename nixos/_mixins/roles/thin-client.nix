{ config, hostname, lib, ... }:

let
  thinClients = import ../../../hosts/thin-clients.nix;

  # Public half of the key the builder signs closures with, written next to the
  # private half by that secret's agenix generator. Committed in the clear
  # because the client has to trust it at evaluation time -- it is a public key,
  # exactly like secrets/masterKeys/*.pub.
  cacheKeyFile = ../../../secrets/store/romeo/nix_cache_key.pub;
  cachePublicKey =
    if builtins.pathExists cacheKeyFile
    then lib.fileContents cacheKeyFile
    else "";
in
{
  assertions = [
    {
      assertion = builtins.elem hostname thinClients;
      message = ''
        ${hostname} imports the thin-client role but is not listed in
        hosts/thin-clients.nix, so the builder would never build a closure for
        it and the host would never update. Add it to that list.
      '';
    }
  ];

  services.bcnelson.thinClient = {
    enable = true;
    inherit cachePublicKey;
  };

  # ~2 GB of RAM. Compressed swap in RAM is the difference between a usable
  # desktop session and the OOM killer; the on-disk swap partition disko creates
  # is the backstop behind it.
  zramSwap.enable = lib.mkDefault true;

  # This host never builds, so build inputs and the full nixpkgs manpage set are
  # pure disk cost on a small eMMC.
  documentation.nixos.enable = lib.mkDefault false;

  # Nothing here pulls the flake, so the auto-update path is not just unused but
  # actively wrong: it would try to git pull and nixos-rebuild on a machine that
  # cannot evaluate its own configuration.
  services.bcnelson.autoUpdate.enable = lib.mkForce false;
}
