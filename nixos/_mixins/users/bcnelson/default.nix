{ pkgs, lib, thinClient ? false, ... }:
let
  bcnelson_init_password = "bcnelson_init_password";
in
{
  # The pairing topic for happy-daemon, which only sierra, golf and redo run.
  # It was declared here unconditionally, so every host with this user demanded a
  # rekeyed copy whether or not it could ever use one -- and on a thin client
  # that was the *only* agenix secret, which meant the sole reason a thin client
  # needed rekeying at all was a notification channel for a daemon it will never
  # start. Since rekeying cannot happen on a 2 GB machine (see
  # docs/thin-clients.md), that one unused secret forced a human with a hardware
  # key into the middle of every thin client install.
  #
  # Still worth dropping even though thin clients now do carry one secret (the
  # tailscale autoconnect's ntfy topic, which buys remote bring-up). Paying a
  # rekey for a channel this host would actually use is a trade; paying one for
  # a daemon it can never start is not.
  age.secrets = lib.optionalAttrs (!thinClient) {
    happy_ntfy_topic = {
      rekeyFile = ../../../../secrets/store/ntfy_topic.age;
      owner = "bcnelson";
      mode = "0400";
    };
  };

  users.users.bcnelson = {
    # TODO: make this more generic
    isNormalUser = true;
    description = "Bradley Nelson";
    extraGroups = [ "networkmanager" "wheel" "plugdev" "docker" "dialout" ];
    # The install script will change mark this user as needing a password change on first login.
    # Note SDDM does not support password changes so this will need to be done via the command line.
    initialPassword = bcnelson_init_password;
    packages = with pkgs; [
      vim
    ];
  };
}
