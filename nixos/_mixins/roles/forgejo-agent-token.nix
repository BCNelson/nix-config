_:
{
  # A read-only Forgejo personal access token for git.bcnelson.dev, so coding
  # agents (and bcnelson) can read private repos with `fj` and plain
  # `git clone https://git.bcnelson.dev/...` without a browser login.
  #
  # Consumed by ../../../home-manager/bcnelson/_mixins/forgejo-cli.nix, which
  # hardcodes the default agenix path (/run/agenix/forgejo-agent-token): it is a
  # home-manager module and cannot see config.age.secrets. Keep the name in sync.
  #
  # Hand-entered: a PAT created at https://git.bcnelson.dev/user/settings/applications
  # with read-only scopes (read:repository, read:issue, read:user,
  # read:organization). Keep it read-only -- it is readable by every process
  # running as bcnelson, agents included.
  #
  # Also imported by system-manager/redo-3.nix; keep this module to options the
  # system-manager agenix mixin supports.
  age.secrets.forgejo-agent-token = {
    rekeyFile = ../../../secrets/store/forgejo_agent_token.age;
    owner = "bcnelson";
    mode = "0400";
  };
}
