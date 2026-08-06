# Hosts that cannot evaluate or build their own system closure (Dell thin
# clients, ~2 GB RAM). Romeo builds these immediately after its own auto-update
# lands, signs them, and publishes them to the nix binary cache; the client only
# ever fetches a store path and switches to it. It never clones the repo, never
# runs nix eval, and never runs nixos-rebuild.
#
# `install-system --thin` appends to this list. The thin-client role asserts
# that its own hostname appears here, so the list and the host's imports cannot
# silently drift apart -- CI catches it via the per-host build check.
# Empty: the wyse-1/wyse-2 pair was only ever a test of this path and has been
# removed. An empty list disables the builder on romeo entirely (see the
# `lib.mkIf (thinClients != [ ])` in nixos/romeo/services/thinClientBuilder.nix),
# so the machinery stays in place, dormant, until a real thin client shows up.
[
]
