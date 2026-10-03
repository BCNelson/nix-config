{ lib, ... }:

# whiskey is a 4-core/4G VPS. It can evaluate its own configuration and switch
# to it, but compiling anything substantial here is hours of thrashing at best
# and the OOM killer at worst (nodejs/V8 is the recurring offender). So it keeps
# driving its own rebuilds and hands the compiling to romeo -- 32 cores, 94G --
# which copies the finished outputs back.
#
# The builder half is nixos/romeo/remote-builder.nix; see docs/remote-builds.md.

let
  # Reached over the tailnet: romeo.b is romeo's tailnet address (main.tf), which
  # is also how this host already scrapes its node exporter. Port 22 there is
  # Tailscale SSH, not romeo's sshd -- so the credential is whiskey's tailnet
  # identity and there is no key, no authorized_keys and no extra port anywhere.
  # tailscale-acl.hujson grants it: `tag:server -> tag:server` in the ssh
  # section (action accept, so no browser re-auth) plus whiskey -> romeo:22 in
  # the acls section.
  builderHost = "romeo.b.nel.family";
  builderUser = "nixremote";
in
{
  nix.distributedBuilds = true;

  nix.buildMachines = [
    {
      hostName = builderHost;
      sshUser = builderUser;
      # No sshKey: Tailscale SSH authenticates the tailnet node, not a key.
      protocol = "ssh-ng";
      systems = [ "x86_64-linux" ];
      # Half of romeo's 32 cores: enough parallelism that nix keeps handing work
      # over instead of falling back to a local slot, while leaving romeo
      # responsive for everything else it runs.
      maxJobs = 16;
      # romeo is the only builder, so this only has to beat the local machine.
      speedFactor = 20;
      supportedFeatures = [ "big-parallel" "benchmark" "kvm" "nixos-test" ];
    }
  ];

  nix.settings = {
    # Let romeo fetch build inputs from its own substituters rather than having
    # whiskey download them just to upload them again over the same tailnet.
    builders-use-substitutes = true;

    # This is what actually keeps the heavy builds off whiskey. Claiming no
    # system features means every derivation with requiredSystemFeatures
    # (big-parallel covers nodejs/V8/chromium) *cannot* be built here, so nix
    # must hand it to romeo. If romeo is unreachable such a build fails with a
    # clear "required system features" error instead of quietly OOM-killing the
    # box; small featureless derivations still build locally, so a builder
    # outage degrades rather than blocks.
    system-features = lib.mkForce [ ];
    # common.nix appends uid-range for nixpkgs tests; keep those on romeo too.
    extra-system-features = lib.mkForce [ ];
  };

  # Tailscale SSH presents tailscaled's own host key, which is in no repo and is
  # regenerated if the node's state is ever rebuilt -- so there is nothing to
  # pin, and a stale pin would silently stop every build on this host. The
  # tailnet has already authenticated romeo (node key plus ACL) before a byte of
  # SSH is spoken, which is why `tailscale ssh` checks no host key either.
  # Scoped to this one destination; LogLevel keeps the unavoidable
  # "known hosts" chatter out of every rebuild log.
  programs.ssh.extraConfig = ''
    Match host ${builderHost} user ${builderUser}
      StrictHostKeyChecking no
      UserKnownHostsFile /dev/null
      LogLevel ERROR
    Match all
  '';
}
