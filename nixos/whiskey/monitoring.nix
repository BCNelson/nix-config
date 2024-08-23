{ config, ... }: {
  # grafana configuration
  services.grafana = {
    enable = true;
    domain = "grafana.b.nel.family";
    port = 2342;
    addr = "127.0.0.1";
  };

  # nginx reverse proxy
  services.nginx.virtualHosts.${config.services.grafana.domain} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString config.services.grafana.port}";
      proxyWebsockets = true;
    };
  };

  services.prometheus = {
    enable = true;
    port = 9001;
  };
}
