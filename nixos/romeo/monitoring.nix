{ config, ... }: {
  services.prometheus = {
    exporters = {
      node = {
        enable = true;
        enabledCollectors = [ "systemd" ];
        port = 9002;
        openFirewall = true;
        firewallFilter = "-i tailscale0 -p tcp --dport 9002 -j ACCEPT";
      };
    };
  };
}