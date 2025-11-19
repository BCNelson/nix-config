_:

{
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    matchBlocks = {
      # "*.b.nel.family" = {
      # };
    };
    addKeysToAgent = true;
  };
}
