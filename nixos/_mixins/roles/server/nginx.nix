{ config, ... }:

{
  age.secrets.porkbun_api_creds.rekeyFile = ../../../../secrets/store/porkbun_api_creds.age;

  security.acme = {
    acceptTerms = true;
    defaults = {
      email = "admin@nel.family";
      dnsProvider = "porkbun";
      environmentFile = config.age.secrets.porkbun_api_creds.path;
    };
  };

  services.nginx = {
    enable = true;
    recommendedGzipSettings = true;
    recommendedZstdSettings = true;
    recommendedTlsSettings = true;
    recommendedProxySettings = true;
    recommendedOptimisation = true;
  };
}
