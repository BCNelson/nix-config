{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm_1_5;
    enableClient = true;
    clientSettings = {
      uri = "https://idm.nel.family";
    };
  };
}
