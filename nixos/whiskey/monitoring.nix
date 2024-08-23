{ config, ... }: {
  # grafana configuration
  services.grafana = {
    enable = true;
    settings.server = {
      root_url = "https://grafana.b.nel.family";
      enable_gzip = true;
      enforce_domain = true;
      domain = "grafana.b.nel.family";
      http_port = 2342;
      http_addr = "127.0.0.1";
    };
  };

  # nginx reverse proxy
  services.nginx.virtualHosts.${config.services.grafana.domain} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString config.services.grafana.settings.server.http_port}";
      proxyWebsockets = true;
    };
  };

  services.prometheus = {
    enable = true;
    port = 9001;
    exporters = {
      node = {
        enable = true;
        enabledCollectors = [ "systemd" ];
        port = 9002;
      };
    };
    scrapeConfigs = [
      {
        job_name = "whiskey";
        static_configs = [
          {
            targets = [ "127.0.0.1:9002" ];
          }
        ];
      }
      {
        job_name = "romeo";
        static_configs = [
          {
            targets = [ "romeo.b.nel.family:9002" ];
          }
        ];
      }
    ];
  };
}
