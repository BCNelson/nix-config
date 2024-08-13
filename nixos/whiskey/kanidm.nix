{ config, ... }:
{
  services.nginx = {
    enable = true;
    virtualHosts = {
      "idm.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 512M;
        '';
        locations = {
          "/" = {
            proxyPass = "https://localhost:3001";
          };
        };
      };
    };
  };

  users.groups.kanidm = {
    members = [ "nginx" ];
  };

  security.acme = {
    certs."idm.nel.family" = {
      group = "kanidm";
    };
  };

  services.kanidm = {
    enableServer = true;
    serverSettings = {
      origin = "https://idm.nel.family";
      domain = "nel.family";
      tls_key = config.security.acme.certs."idm.nel.family".directory + "/key.pem";
      tls_chain = config.security.acme.certs."idm.nel.family".directory + "/fullchain.pem";
      role = "WriteReplica";
      bindaddress = "127.0.0.1:3001";
    };
    enableClient = true;
    clientSettings = {
      uri = "https://127.0.0.1:3001";
    };
  };
}
