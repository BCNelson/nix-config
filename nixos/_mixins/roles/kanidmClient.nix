{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm_1_4;
    enableClient = true;
    clientSettings = {
      uri = "https://idm.nel.family";
    };
  };
}
