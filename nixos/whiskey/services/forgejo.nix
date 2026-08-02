{ config, ... }:
let
  dataDirs = {
    level3 = "/data/level3"; # High
  };
in
{
  age.secrets.cloudflare_dns_api_token.rekeyFile = ../../../secrets/store/cloudflare_dns_api_token.age;

  age-template.files."cloudflare-acme-env" = {
    vars.token = config.age.secrets.cloudflare_dns_api_token.path;
    content = "CF_DNS_API_TOKEN=$token";
  };

  security.acme.certs."git.bcnelson.dev" = {
    dnsProvider = "cloudflare";
    environmentFile = config.age-template.files."cloudflare-acme-env".path;
  };
  services.nginx = {
    enable = true;
    virtualHosts = {
      "git.bcnelson.dev" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 512M;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:3000";
          };
        };
      };
    };
  };

  services.forgejo = {
    enable = true;
    # Enable support for Git Large File Storage
    lfs.enable = true;
    settings = {
      server = {
        DOMAIN = "git.bcnelson.dev";
        # You need to specify this to remove the port from URLs in the web UI.
        ROOT_URL = "https://git.bcnelson.dev/";
        HTTP_PORT = 3000;

        # Git over SSH shares port 22 with the host sshd (Forgejo's default:
        # START_SSH_SERVER stays false). sshd authenticates the `forgejo` user
        # against ~forgejo/.ssh/authorized_keys, which Forgejo rewrites whenever a
        # user adds a key; each entry carries a forced `forgejo serv key-N`
        # command that identifies the pusher. Admin logins are ordinary sshd
        # accounts on the same port.
        #
        # This coexists with Tailscale SSH (`tailscale up --ssh` in
        # roles/tailscale.nix) because tailscaled only intercepts port 22 on the
        # TAILNET address. git.bcnelson.dev resolves to the PUBLIC address, so git
        # traffic reaches the real sshd and never meets the interception —
        # including from machines that are themselves on the tailnet.
        #
        # The one combination that cannot work is addressing Forgejo over the
        # tailnet directly (ssh://forgejo@whiskey-1/... or the 100.x address):
        # Tailscale SSH authenticates by tailnet identity and runs the command
        # itself, so the `forgejo serv key-N` forced command never fires and
        # Forgejo cannot tell who is pushing. Always clone via the domain.
        #
        # Clone URLs are ssh://forgejo@git.bcnelson.dev/<owner>/<repo>.git
      };
      # You can temporarily allow registration to create an admin user.
      service.DISABLE_REGISTRATION = true;
      # Add support for actions, based on act: https://github.com/nektos/act
      actions = {
        ENABLED = true;
        DEFAULT_ACTIONS_URL = "github";
      };
    };
    dump = {
      enable = true;
      type = "tar.zst";
      backupDir = "${dataDirs.level3}/forgejo";
    };
  };
}
