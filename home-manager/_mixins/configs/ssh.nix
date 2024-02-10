_:

{
  programs.ssh = {
    enable = true;
    matchBlocks = {
      # "*.b.nel.family" = {
      # };
    };
    addKeysToAgent = true;
  };

  
}
