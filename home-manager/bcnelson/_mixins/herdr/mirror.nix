{ hostname, lib, pkgs, ... }:

# herdr-mirror pulls a remote herdr server's workspaces and agents into the
# local sidebar, so one window shows every machine's agents -- blocked, working,
# done -- with live panes you can type into.
#
# This is a local-client feature: the remote end needs nothing but herdr, which
# ../../../_mixins/roles already puts on every host system-wide. So it belongs
# here in the workstation half of the herdr mixin rather than in ./core.nix,
# which every non-thin host takes.

let
  inherit (pkgs) herdr-mirror;

  pluginRoot = "${herdr-mirror}/${herdr-mirror.pluginRoot}";

  # Tailscale addresses rather than hostnames. b.nel.family only carries records
  # for romeo, whiskey and charlie (main.tf), and neither bare hostnames nor
  # MagicDNS names resolve from redo-3 -- an address is the only form that works
  # from every workstation this mixin lands on. Same values as
  # tailscale-acl.hujson's `hosts` block, which is where they are otherwise
  # written down; they are node addresses, so they only change if a node is
  # removed from the tailnet and re-joined.
  hosts = {
    romeo-2.target = "100.76.49.168";
    whiskey-1.target = "100.89.15.100";

    sierra-2 = {
      target = "100.92.32.99";

      # A machine with its own display and someone sitting at it. The default
      # (always_control) keeps a mirror writable and drives the remote pane to
      # the local pane's size, which is right for a headless server filling your
      # window and wrong for a desktop whose own window would get resized under
      # its user. Read-only here instead: the mirror escalates to control when
      # you actually type, and releases after an hour idle.
      always_control = false;
    };
  };

  # A workstation must not mirror itself: the daemon would ssh back into this
  # machine and mirror its own workspaces, which recurses in the sidebar.
  mirrored = lib.filterAttrs (name: _: name != hostname) hosts;

  # `herdr plugin install` git-clones the plugin, runs its build step, and
  # records the result in ~/.config/herdr/plugins.json. The store has no room
  # for the first two, so declare the registry entry the installer would have
  # written -- generated from the packaged manifest rather than transcribed, so
  # an upstream version that adds an action does not need this file touched.
  #
  # The trade-off of owning plugins.json: it becomes a read-only symlink, so
  # `herdr plugin install/uninstall/enable/disable` can no longer write it. A
  # second plugin gets added here, the same way.
  pluginRegistry = pkgs.runCommand "herdr-plugins.json"
    {
      nativeBuildInputs = [ pkgs.python3 ];
      inherit pluginRoot;
    } ''
    python3 ${./plugin-registry.py} > "$out"
  '';
in
{
  home.packages = [ herdr-mirror ];

  xdg.configFile = {
    "herdr/plugins.json".source = pluginRegistry;

    # The canonical location of the two the plugin searches; the other is
    # `herdr plugin config-dir mirror`, which would shadow this one. Left alone
    # so `herdr-mirror status` typed in a shell reads the same config the
    # autostart hook does.
    #
    # default_host is deliberately unset. It only matters for
    # remote-new-workspace invoked outside a mirror, and the plugin's own
    # default -- the first host -- is already the answer we would write, without
    # the edge case of naming the host this file happens to be evaluated on.
    "herdr-mirror/hosts.toml".source =
      (pkgs.formats.toml { }).generate "herdr-mirror-hosts.toml" { hosts = mirrored; };
  };

  # The daemon warns (and toasts) when this link is missing, because keybindings
  # bound to a bare `herdr-mirror` cannot fire without it. Nothing here binds it
  # that way -- ./core.nix keybindings carry absolute store paths -- but the link
  # is what makes the CLI usable by hand, so declare it rather than let
  # `herdr-mirror start` write it and go stale on the next update. `status`
  # reports it as "not this binary" because it resolves to the PATH wrapper
  # rather than the wrapped binary the daemon runs as; that state is explicitly
  # left alone, never repaired.
  home.file.".local/bin/herdr-mirror".source = lib.getExe herdr-mirror;

  # Plugin actions have no default keys. `herdr-mirror bind` would write these
  # into config.toml itself, but config-merge renders keys.command from
  # ./core.nix and declarative wins, so anything it wrote there is discarded on
  # the next reconcile -- these belong in Nix.
  #
  # The remote-* four are herdr's native local key plus alt: same muscle memory,
  # remote target. Outside a mirror they fall back to the plain local action, so
  # nothing is lost by pressing them on a local pane.
  local.herdr.extraCommandKeys = [
    {
      key = "prefix+shift+m";
      type = "plugin_action";
      command = "mirror.start";
    }
    {
      # "bring back" -- prefix+shift+r is herdr's own reload_config.
      key = "prefix+shift+b";
      type = "plugin_action";
      command = "mirror.restore";
    }
    {
      # native new_workspace = prefix+shift+n
      key = "prefix+alt+n";
      type = "plugin_action";
      command = "mirror.remote-new-workspace";
    }
    {
      # native new_tab = prefix+c
      key = "prefix+alt+c";
      type = "plugin_action";
      command = "mirror.remote-new-tab";
    }
    {
      # native split_vertical = prefix+v
      key = "prefix+alt+v";
      type = "plugin_action";
      command = "mirror.remote-split-right";
    }
    {
      # native split_horizontal = prefix+minus
      key = "prefix+alt+minus";
      type = "plugin_action";
      command = "mirror.remote-split-down";
    }
  ];
}
