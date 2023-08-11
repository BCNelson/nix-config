# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ outputs, hostname, ... }:

let
    # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
    hostnamePrefix = builtins.substring 0 (builtins.indexOf "-" hostname);
in
{
  imports = []; ++ lib.optional (builtins.isPath ./${hostnamePrefix}) [./${hostnamePrefix}];
}
