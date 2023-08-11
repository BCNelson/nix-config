# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ outputs, hostname, lib, stateVersion, ... }:

let
    # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
    hostnamePrefix = builtins.substring 0 (builtins.indexOf "-" hostname);
in
{
  imports = [ ./common.nix ] ++ lib.optional (builtins.isPath ./${hostnamePrefix}) ./${hostnamePrefix}
  ++ lib.optional (builtins.isPath ./hosts/${hostname}) ./hosts/${hostname};
  networking.hostName = hostname;
  system.stateVersion = stateVersion;
}
