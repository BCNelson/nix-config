{ ... }:
{
  imports = [
    # Pull-only updates: this host never clones the repo, never evaluates its
    # own configuration and never rebuilds. Romeo builds the closure and
    # publishes it; the thin-client role fetches and activates it.
    ../_mixins/roles/thin-client.nix
  ];
}
