{ ... }:
{
  # Nothing to import: listing this host in hosts/thin-clients.nix is what makes
  # it a thin client. nixos/default.nix reads that registry and pulls in
  # _mixins/roles/thin-client, so the registry and the host's imports cannot
  # disagree. Put host-specific configuration here.
  imports = [ ];
}
