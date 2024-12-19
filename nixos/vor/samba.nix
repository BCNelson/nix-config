_:
{
  services.samba-wsdd.enable = true; # make shares visible for windows 10 clients
  networking.firewall.allowedTCPPorts = [
    5357 # wsdd
  ];
  networking.firewall.allowedUDPPorts = [
    3702 # wsdd
  ];
  services.samba = {
    enable = true;
    securityType = "user";
    openFirewall = true;
    settings.global = {
      "workgroup" = "WORKGROUP";
      "server string" = "vor";
      "netbios name" = "vor";
      "security" = "user";
      "#use sendfile" = "yes";
      "min protocol" = "smb3";
      "hosts allow" = "100.64.0.0/10 192.168.138.0/24";
      "hosts deny" = "0.0.0.0/0";
      "guest account" = "nobody";
      "map to guest" = "bad user";
    };
    shares = {
      family = {
        path = "/liveData/NelsonData/Nelson Family Files";
        browseable = "yes";
        "read only" = "no";
        "guest ok" = "yes";
        "create mask" = "0644";
        "directory mask" = "0755";
        "force user" = "samba";
        "force group" = "samba";
      };
    };
  };

  users.users.samba = {
    isNormalUser = false;
    isSystemUser = true;
    description = "Samba";
    group = "samba";
  };

  users.groups.samba = {};
}
