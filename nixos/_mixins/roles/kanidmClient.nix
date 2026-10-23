{ pkgs, ... }:
{
  services.kanidm = {
    # Only the CLI; unlike the server (whiskey), it has no migration to step
    # through, so it can go straight to the current version.
    package = pkgs.kanidm_1_11;
    client.enable = true;
    client.settings = {
      uri = "https://idm.nel.family";
    };
  };
}
