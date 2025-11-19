{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm_1_7;
    enableClient = true;
    clientSettings = {
      uri = "https://idm.nel.family";
    };
  };
}
