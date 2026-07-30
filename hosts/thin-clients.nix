# Hosts that cannot evaluate or build their own system closure (Dell thin
# clients, ~2 GB RAM). Romeo builds these immediately after its own auto-update
# lands, signs them, and publishes them to the nix binary cache; the client only
# ever fetches a store path and switches to it. It never clones the repo, never
# runs nix eval, and never runs nixos-rebuild.
#
# `install-system --thin` appends to this list. The thin-client role asserts
# that its own hostname appears here, so the list and the host's imports cannot
# silently drift apart -- CI catches it via the per-host build check.
[
  "wyse-1"
]
