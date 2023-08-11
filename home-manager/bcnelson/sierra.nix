{ config, pkgs, outputs, stateVersion, ... }:

{
  imports = [
    ./_mixins/programs/deckmaster
  ];
}
