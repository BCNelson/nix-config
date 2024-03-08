_:
{
  networking.useNetworkd= true;
  systemd.network = {
    enable = true;
    netdevs = {
      "20-vlan10" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan10";
        };
        vlanConfig.Id = 10;
      };
      "20-vlan30" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan30";
        };
        vlanConfig.Id = 30;
      };
    };
    networks = {
      "30-enp11s0" = {
        matchConfig.Name = "enp11s0";
        vlan = [
          "vlan10"
          "vlan30"
        ];
        networkConfig.LinkLocalAddressing = "no";
        linkConfig.RequiredForOnline = "carrier";
      };
      "40-vlan10" = {
        matchConfig.Name = "vlan10";
        addresses = [
          "10.10.1.7/24"
        ];
      };
      "40-vlan30" = {
        matchConfig.Name = "vlan30";
        addresses = [
          "10.30.1.7/24"
        ];
      };
    };
  };
}
