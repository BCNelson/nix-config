{ pkgs, ... }:
{
  services.kanidm = {
    package = pkgs.kanidm;
    enableClient = true;
    clientSettings = {
      uri = "https://idm.nel.family";
    };
  };
}
