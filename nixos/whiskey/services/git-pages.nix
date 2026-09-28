{ pkgs, ... }:
let
  # Default pages domain. <repo>.bcnelson.page serves the `pages` branch of
  # git.bcnelson.dev/bcnelson/<repo>.
  pagesDomain = "bcnelson.page";

  # git-pages listens on loopback only; nginx terminates TLS and is the only
  # way in. The upstream default is 3000, which Forgejo already owns here.
  pagesPort = 3300;

  proxyToPages = {
    proxyPass = "http://127.0.0.1:${toString pagesPort}";
    extraConfig = ''
      # A publish is a PUT of the whole site as a tar, not a form post.
      client_max_body_size 256M;
      # Stream it through to git-pages rather than spooling it onto whiskey's
      # small root disk first.
      proxy_request_buffering off;
      # git-pages-cli refuses to publish unless the response `Server:` (or
      # `X-Server:`) header matches /\bgit-pages\b/ — and for the PUT that
      # publishes, that is a hard error, not a warning. nginx does not forward
      # an upstream Server header by default, so without this the CLI sees
      # "nginx" and exits 1.
      proxy_pass_header Server;
    '';
  };
in
{
  # Replaces the bespoke homefirst-pages puller with the service Codeberg Pages
  # itself runs. Forgejo has no Pages feature and has declared it out of scope
  # (forgejo/forgejo#2708), so this is the supported way to get one.
  #
  # git-pages keeps no accounts and stores no credentials. A publish is a PUT of
  # a tar carrying a `Forge-Authorization:` header, which git-pages replays to
  # Forgejo as `Authorization:` against
  #   GET /api/v1/repos/<owner>/<repo>   -> requires .permissions.push
  #   GET /api/v1/user                   -> recorded in the audit log
  # so the right to publish is exactly the right to push, and CI authenticates
  # with the per-run token Forgejo mints for it (forgejo.token). Nothing here
  # needs an agenix secret.
  #
  # Both API calls share a 5s timeout: if Forgejo is restarting, a publish fails
  # with 503 "cannot check repository permissions" rather than an auth error.

  system.services.git-pages = {
    imports = [ pkgs.git-pages.services.default ];

    git-pages.settings = {
      server = {
        pages = "tcp/127.0.0.1:${toString pagesPort}";
        # The `caddy` listener exists to answer Caddy's on-demand-TLS
        # permission queries. nginx holds every certificate here, and
        # bcnelson.dev/.page carry a CAA record restricting issuance to
        # dns-01, which on-demand TLS cannot satisfy. Nothing to answer.
        caddy = "-";
        metrics = "-";
      };

      # <repo>.bcnelson.page -> git.bcnelson.dev/bcnelson/<repo> @ `pages`.
      #
      # The owner is pinned in clone-url and `index-repo = "<user>"` templates
      # the *hostname label* into the repo name, which is what keeps these URLs
      # one label deep instead of <owner>.<domain>/<repo>/. Consequences worth
      # knowing before adding a second tier:
      #   - the subdomain must name an existing repo under `bcnelson`;
      #   - another owner cannot publish here at all without its own
      #     [[wildcard]] section, plus its own wildcard CNAME and certificate
      #     (DNS and TLS wildcards each match exactly one label);
      #   - the apex `bcnelson.page` is NOT covered: a wildcard pattern
      #     requires at least one label more than its domain. A site there
      #     would need a `_git-pages-forge-allowlist` TXT record, the way
      #     homefirst.dev below does.
      wildcard = [
        {
          domain = pagesDomain;
          "clone-url" = "https://git.bcnelson.dev/bcnelson/<project>.git";
          "index-repo" = "<user>";
          "index-repo-branch" = "pages";
          authorization = "forgejo";
        }
      ];

      # Site data lives under /var/lib/git-pages (the module's default, and
      # DynamicUser makes anything outside StateDirectory awkward). Cap each
      # site so a runaway build can't eat the root filesystem.
      limits."max-site-size" = "256M";
    };
  };

  # One certificate for the wildcard and the apex, issued over DNS-01 with the
  # porkbun credentials from roles/server/nginx.nix — bcnelson.page is
  # registered there, so no per-cert provider override is needed.
  # A vhost using `enableACME` gets this group automatically; one using
  # `useACMEHost` has to say so, or nginx cannot read the key.
  security.acme.certs.${pagesDomain} = {
    domain = "*.${pagesDomain}";
    extraDomainNames = [ pagesDomain ];
    group = "nginx";
  };

  services.nginx = {
    enable = true;
    virtualHosts = {
      ${pagesDomain} = {
        serverAliases = [ "*.${pagesDomain}" ];
        useACMEHost = pagesDomain;
        forceSSL = true;
        locations."/" = proxyToPages;
      };

      # homefirst.dev is published as a *custom domain*, not through the
      # wildcard: the site lives at the apex, and a [[wildcard]] section can
      # never match its own apex. Authorization comes from the
      # _git-pages-forge-allowlist.homefirst.dev TXT record (see main.tf),
      # which names the clone URL allowed to publish here; git-pages then still
      # checks push permission against Forgejo. That record authorizes the
      # index site only, which is all an apex site needs.
      "homefirst.dev" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null; # DNS-01
        locations."/" = proxyToPages;
      };

      # git-pages keys sites on the Host header, so www.homefirst.dev would be
      # a separate (empty) site rather than an alias — and with no wildcard
      # section for this zone it cannot be published at all. Redirect instead.
      "www.homefirst.dev" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null; # DNS-01
        globalRedirect = "homefirst.dev";
      };
    };
  };
}
