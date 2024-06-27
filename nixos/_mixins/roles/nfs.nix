_: {
  services.nfs.idmapd.settings = {
    General = {
      Domain = "nel.family";
      Method = "nsswitch";
    };
  };
}
