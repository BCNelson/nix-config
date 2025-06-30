{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm_1_6;
    enableClient = true;
    clientSettings = {
      uri = "https://idm.nel.family";
    };
  };
}
