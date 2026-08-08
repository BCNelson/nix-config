{ hostname, lib, ... }:

let
  thinClients = import ../../../../hosts/thin-clients.nix;

  # Public half of the key the builder signs closures with, written next to the
  # private half by that secret's agenix generator. Committed in the clear
  # because the client has to trust it at evaluation time -- it is a public key,
  # exactly like secrets/masterKeys/*.pub.
  cacheKeyFile = ../../../../secrets/store/romeo/nix_cache_key.pub;
  cachePublicKey =
    if builtins.pathExists cacheKeyFile
    then lib.fileContents cacheKeyFile
    else "";
in
{
  # The cut-down base every thin client gets. Kept separate because it is about
  # what this class of machine *is* (an appliance with no build capability),
  # while the rest of this file is about how it updates.
  imports = [
    ./minimal.nix
    ./low-memory.nix
    ./firmware.nix
    ../../hardware/emmc.nix
    # Reaching a headless appliance that lives on someone else's LAN is the
    # whole point of having it on the tailnet, and the autoconnect script is
    # what makes bringing one up remote-friendly: plug it in, the auth URL
    # arrives as a phone notification, tap it, done. No console, no hunting for
    # a DHCP lease, no password -- which matters because there are no
    # authorized_keys anywhere on this fleet, so until a host is on the tailnet
    # the only ways in are the console and a password.
    #
    # This is the one thing that gives a thin client an agenix secret (the ntfy
    # topic), and so the one thing that costs a rekey per new host. That trade
    # was made deliberately -- see "Rekeying" in docs/thin-clients.md.
    ../tailscale.nix
  ];

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

  # A headless appliance with no root password cannot use systemd's initrd
  # emergency console at all: it asks to authenticate against an account that is
  # locked, so a failed boot ends at "cannot open access to console" and the
  # only way in is to walk over with the installer ISO. Learned the hard way.
  #
  # The eMMC is unencrypted, so anyone who can reach the console can already
  # read the disk; a shell there grants nothing that physical access did not
  # already imply, and it turns an unbootable appliance into one you can
  # diagnose in place.
  boot.initrd.systemd.emergencyAccess = true;

  # Nothing here pulls the flake, so the auto-update path is not just unused but
  # actively wrong: it would try to git pull and nixos-rebuild on a machine that
  # cannot evaluate its own configuration.
  services.bcnelson.autoUpdate.enable = lib.mkForce false;

  # An appliance is not where you drive a coding agent from. Stated rather than
  # merely left off, because its ntfy secret was the one thing forcing a rekey
  # on to a machine that cannot do one -- see the bcnelson user mixin.
  services.bcnelson.happy-daemon.enable = lib.mkForce false;
}
