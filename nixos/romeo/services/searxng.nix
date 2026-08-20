{
  config,
  pkgs,
  ...
}: let
  # SearXNG's own default, and free on romeo. Kept explicit because
  # ./openclaw.nix has to name the same number on the other side and a silent
  # upstream default change would break the pair.
  port = 8888;
in {
  # SearXNG exists on this host for exactly one consumer: the `web_search` tool
  # in ./openclaw.nix. It is a metasearch proxy -- it holds no index of its own,
  # it fans a query out to Google/Bing/DuckDuckGo/etc. and merges the results.
  #
  # Chosen over the API-backed alternatives (Brave, Tavily, Exa, Perplexity) for
  # the same reason ./ollama.nix carries the dreaming model: no third-party
  # account, no key to mint by hand outside the repo, no per-query billing, and
  # the upstream engines see romeo's address rather than an identity tied to a
  # subscription. It is also the only key-free provider openclaw will select by
  # auto-detection -- DuckDuckGo and the other key-free options are documented as
  # never winning auto-detect, so they have to be pinned explicitly anyway.
  #
  # Deliberately NOT reverse-proxied. Every other web service on this host gets
  # an nginx vhost; this one has no human user, so there is nothing to serve and
  # no allowlist to maintain. Adding a vhost later is the usual four lines plus
  # `server.base_url`, but until something other than openclaw wants it, the
  # smaller attack surface is the better default for a service whose whole job is
  # fetching attacker-influenced content off the public internet.

  # Never typed by a human and never read back out, so it is generated rather
  # than stored -- same pattern as openclaw's gateway token in ./openclaw.nix,
  # minus the Bitwarden entry, because there is no flow that asks anyone for it.
  #
  # hex rather than base64 (which ./journiv.nix uses) because this value is
  # substituted by envsubst into a *double-quoted YAML scalar*; base64's `"` is
  # not in its alphabet but its `/` and `+` make the result needlessly
  # quote-sensitive, and hex has no character that YAML or a shell cares about.
  age.secrets.searx-secret-key = {
    rekeyFile = ./secrets/searx_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  # Read by systemd as root before it drops to User=searx, so the default
  # 0400 root:root from agenix-template is correct here and the searx user never
  # needs to be able to read it. The NixOS module wires this same file into both
  # searx-init.service (which runs the envsubst pass) and searx.service.
  age-template.files.searx-env = {
    vars = {
      SECRET_KEY = config.age.secrets.searx-secret-key.path;
    };
    content = ''
      SEARX_SECRET_KEY=$SECRET_KEY
    '';
  };

  services.searx = {
    enable = true;
    environmentFile = config.age-template.files.searx-env.path;

    # configureUwsgi is left at its default of false, so this runs SearXNG's
    # built-in Flask server rather than a uWSGI vassal. The module's own option
    # docs call uWSGI "the recommended mode for public or large instances, but
    # unnecessary for LAN or local-only use" -- this instance is one loopback
    # client issuing a handful of queries per agent turn, which is the case the
    # built-in server is fine for, and it avoids standing up a uwsgi emperor
    # that nothing else on romeo needs.
    #
    # The option docs also warn that "the built-in HTTP server logs all queries
    # by default". See general.debug below for why that does not apply here.

    settings = {
      # Load-bearing for privacy, not just verbosity. SearXNG's logging setup
      # (searx/__init__.py) branches on this value, and the production branch
      # runs `logging.getLogger('werkzeug').setLevel(WARNING)`. Werkzeug's
      # access log is INFO, so that one line is what stops every request from
      # being written to the journal -- and openclaw's requests are GETs of the
      # form `/search?q=<the agent's query>&format=json`, so the access log
      # would put the full text of every search the agent runs into journald.
      #
      # false is also SearXNG's upstream default, so this line changes nothing
      # today. It is stated because the behaviour above is a side effect of a
      # setting whose name suggests it only affects log *level*, and a future
      # "turn on debug to investigate something" would silently start recording
      # queries. Turn it on to debug, turn it back off afterwards.
      general.debug = false;

      server = {
        inherit port;
        # Also the upstream default. Stated because it is the entire security
        # model of this service: there is no vhost, no auth and no rate limit in
        # front of it, so nothing but the bind address keeps it off the LAN.
        bind_address = "127.0.0.1";

        # Substituted by envsubst from environmentFile above, in the settings.yml
        # that searx-init.service materialises under /run/searx. Not optional:
        # searx/webapp.py's init() does
        #   if not app.debug and get_setting("server.secret_key") == 'ultrasecretkey':
        #       logger.error(...); sys.exit(1)
        # so leaving it at the packaged default is a hard startup failure rather
        # than a warning.
        secret_key = "$SEARX_SECRET_KEY";

        # Off is the upstream default; stated because turning it on would break
        # this instance's only client. The limiter is bot *detection* -- it
        # scores requests on cookies, Accept headers and User-Agent, all of which
        # a plain JSON API call from openclaw fails. It also requires a valkey
        # server (services.searx.redisCreateLocally), which is why it cannot be
        # enabled by accident. Access control here is bind_address, above.
        limiter = false;
      };

      search = {
        # The reason this instance is usable at all. SearXNG ships
        # `formats: [html]`, and searx/webapp.py rejects any other output with a
        # bare `flask.abort(403)`:
        #   if output_format not in settings['search']['formats']: flask.abort(403)
        # openclaw's plugin requests `format=json` and surfaces the result as
        # "SearXNG search error (403)", which reads like an auth problem and is
        # not one.
        #
        # This replaces rather than extends the upstream list: SearXNG's settings
        # merge (searx/settings_loader.py update_dict) recurses into mappings but
        # assigns lists wholesale, so "html" has to be repeated here to keep it.
        formats = ["html" "json"];
      };
    };
  };
}
