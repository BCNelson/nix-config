_:

{
  programs.ssh = {
    enable = true;
    matchBlocks = {
      # "*.b.nel.family" = {
      #   extraOptions = {
      #     "RemoteCommand" = "fish";
      #   };
      # };
    };
  };
}
