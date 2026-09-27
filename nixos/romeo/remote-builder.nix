{ config, pkgs, ... }:

# romeo compiles for hosts that can evaluate and switch their own configuration
# but have no business running a compiler. This is the other half of
# nixos/whiskey/remote-builds.nix: whiskey evaluates, ships the derivations here
# over Tailscale SSH, romeo builds them, and whiskey copies the outputs back.
#
# Distinct from the thin-client builder (./services/thinClientBuilder.nix), which
# builds a *whole system* on a client's behalf and publishes it to the binary
# cache. That is for hosts that cannot even evaluate; this is for hosts that can
# drive their own rebuild but not finish it.
#
# Nothing here opens a port or installs a key. Transport and authorization are
# Tailscale's: tailscaled owns port 22 on the tailnet address, and
# tailscale-acl.hujson admits tag:server as this one account and no other.

let
  # Tailscale SSH has no authorized_keys, so there is nowhere to hang the
  # `command="..."` that would normally pin a build account to one program. The
  # login shell is the only place left, so this is that forced command: the
  # account speaks the nix remote-build protocol and can run nothing else. A
  # compromised whiskey gets a daemon connection, not a shell inside the LAN.
  #
  # Exact matches, and it execs a fixed argv rather than the string it was
  # handed. A prefix match into `bash -c` would happily carry anything appended
  # after the part that matched, which is the usual way a forced command turns
  # out not to be one. Measured, not assumed: nix joins the remote command with
  # plain spaces and does not shell-escape it, and tailscaled invokes the
  # account's login shell as `<shell> -c "<command>"`.
  #
  # If a future nix changes that invocation -- an added `--store`, say -- builds
  # fail with the refusal below naming the command it wanted, which is the whole
  # diagnosis.
  buildShell = pkgs.writeShellApplication {
    name = "nix-remote-build-shell";
    text = ''
      cmd=""
      while [ "$#" -gt 0 ]; do
        if [ "$1" = "-c" ]; then
          cmd="''${2-}"
          break
        fi
        shift
      done

      case "$cmd" in
        # ssh-ng, which is what nixos/whiskey/remote-builds.nix asks for.
        "nix daemon --stdio")
          exec ${config.nix.package}/bin/nix daemon --stdio
          ;;
        # The older ssh:// protocol, in case a client ever falls back to it.
        "nix-store --serve --write")
          exec ${config.nix.package}/bin/nix-store --serve --write
          ;;
      esac

      echo "nixremote speaks the nix remote-build protocol and nothing else." >&2
      echo "refused: ''${cmd:-<interactive shell>}" >&2
      echo "see docs/remote-builds.md" >&2
      exit 1
    '';
  };
in
{
  users.groups.nixremote = { };
  users.users.nixremote = {
    isSystemUser = true;
    group = "nixremote";
    description = "Remote build user for hosts that offload builds to romeo";
    shell = "${buildShell}/bin/nix-remote-build-shell";
    # Tailscale SSH chdirs into the home directory of the account it lands as,
    # and `nix daemon` wants a writable HOME for its own cache.
    home = "/var/lib/nixremote";
    createHome = true;
  };

  # ssh-ng hands the builder derivations and input paths that nothing has
  # signed, and only a trusted user may add those to the store -- without this
  # every remote build fails with "cannot add path ... lacks a valid signature".
  # Merges with the list in nixos/common.nix.
  #
  # This is the real extent of what a build client is trusted with, and it is
  # not small: a trusted user can add arbitrary paths to romeo's store, override
  # daemon settings, and reach the network from a fixed-output derivation. The
  # forced command above keeps whiskey from running commands here; it does not
  # make this a security boundary. Remote building cannot be done without it
  # (see the note on buildMachines.sshUser in the NixOS options).
  nix.settings.trusted-users = [ "nixremote" ];
}
