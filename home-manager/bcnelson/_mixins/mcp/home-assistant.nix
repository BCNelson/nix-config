_: let
  # Home Assistant has no Dynamic Client Registration. It uses IndieAuth: the
  # client_id is a URL, and a redirect_uri on the same scheme+host:port is
  # accepted without HA fetching anything. So pin MCPProxy's loopback callback
  # to a fixed port and use that origin as the client_id.
  callbackOrigin = "http://127.0.0.1:8641";
in {
  services.mcpproxy.upstreams.home-assistant = {
    url = "https://homeassistant.h.b.nel.family/api/mcp";
    oauth = {
      client_id = "${callbackOrigin}/";
      redirect_uri = "${callbackOrigin}/oauth/callback";
      pkce_enabled = true;
    };
  };
}
