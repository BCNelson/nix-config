{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm_1_9;
    client.enable = true;
    client.settings = {
      uri = "https://idm.nel.family";
    };
  };
}
