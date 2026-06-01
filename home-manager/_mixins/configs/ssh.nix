_:

{
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    settings = {
      # "*.b.nel.family" = {
      # };
    };
    addKeysToAgent = true;
  };
}
