{ config, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };
in
{
  age.secrets.cloudflare_dns_api_token.rekeyFile = ../../../secrets/store/cloudflare_dns_api_token.age;

  age-template.files."cloudflare-acme-env" = {
    vars.token = config.age.secrets.cloudflare_dns_api_token.path;
    content = "CF_DNS_API_TOKEN=$token";
  };

  security.acme.certs."git.bcnelson.dev" = {
    dnsProvider = "cloudflare";
    environmentFile = config.age-template.files."cloudflare-acme-env".path;
  };
  services.nginx = {
    enable = true;
    virtualHosts = {
      "git.bcnelson.dev" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 512M;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:3000";
          };
        };
      };
    };
  };

  services.forgejo = {
    enable = true;
    # Enable support for Git Large File Storage
    lfs.enable = true;
    settings = {
      server = {
        DOMAIN = "git.bcnelson.dev";
        # You need to specify this to remove the port from URLs in the web UI.
        ROOT_URL = "https://git.bcnelson.dev/";
        HTTP_PORT = 3000;
      };
      # You can temporarily allow registration to create an admin user.
      service.DISABLE_REGISTRATION = true;
      # Add support for actions, based on act: https://github.com/nektos/act
      actions = {
        ENABLED = true;
        DEFAULT_ACTIONS_URL = "github";
      };
    };
    dump = {
      enable = true;
      type = "tar.zst";
      backupDir = "${dataDirs.level3}/forgejo";
    };
  };
}
