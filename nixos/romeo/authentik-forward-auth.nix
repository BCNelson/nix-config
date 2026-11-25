# Forward auth against authentik on whiskey, for romeo's nginx vhosts whose
# apps have no login of their own (or delegate it to the proxy).
#
# Usage: `locations = (import ./authentik-forward-auth.nix) { "/" = {...}; };`
# The given "/" location is gated: every request to it is checked with an
# auth_request to authentik's embedded outpost, and a 401 bounces the browser to
# the outpost's /start, which runs the authentik login and returns here. Any
# other locations passed in are left as they are (e.g. an API bypass).
#
# Which provider answers: the "arr" forward_domain provider
# (nixos/whiskey/services/authentik/blueprints/arr.yaml). Its session cookie is
# set on nel.family, so it covers every host under it, and who may pass is
# decided by that application's policy bindings (service_admins). A second
# forward_domain provider for the same cookie domain would be ambiguous on the
# outpost, so new vhosts share this one.
#
# The outpost is reached by name at request time (resolver + variable) rather
# than resolved at config load, so nginx still starts if unbound is not up yet
# or whiskey is unreachable; only the gated vhosts fail until it is.
let
  authentikHost = "auth.nel.family";
in
  locations:
    locations
    // {
      "/" =
        locations."/"
        // {
          extraConfig = ''
            ${locations."/".extraConfig or ""}
            auth_request /outpost.goauthentik.io/auth/nginx;
            error_page 401 = @goauthentik_proxy_signin;
            auth_request_set $auth_cookie $upstream_http_set_cookie;
            add_header Set-Cookie $auth_cookie;
          '';
        };
      "/outpost.goauthentik.io" = {
        # recommendedProxySettings is included after extraConfig and would
        # re-set Host to the app's vhost, which whiskey's nginx would route to
        # the wrong server block.
        recommendedProxySettings = false;
        extraConfig = ''
          resolver 127.0.0.1 valid=300s;
          # Every asset on the page triggers an auth subrequest, so an
          # unreachable address family (whiskey once published an AAAA it had
          # not configured) cost the default 60s per request before fallback.
          # Fail over fast instead.
          proxy_connect_timeout 5s;
          set $authentik https://${authentikHost};
          proxy_pass $authentik;
          proxy_ssl_server_name on;
          proxy_set_header Host ${authentikHost};
          proxy_set_header X-Original-URL $scheme://$http_host$request_uri;
          proxy_pass_request_body off;
          proxy_set_header Content-Length "";
          auth_request_set $auth_cookie $upstream_http_set_cookie;
          add_header Set-Cookie $auth_cookie;
        '';
      };
      "@goauthentik_proxy_signin".extraConfig = ''
        internal;
        add_header Set-Cookie $auth_cookie;
        return 302 /outpost.goauthentik.io/start?rd=$scheme://$http_host$request_uri;
      '';
    }
