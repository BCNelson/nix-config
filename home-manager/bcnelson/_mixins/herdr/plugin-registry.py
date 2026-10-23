"""Render herdr's plugins.json from packaged plugin manifests.

herdr keeps its plugin registry as a JSON array of entries, each one the
plugin's herdr-plugin.toml plus where it was installed from. `herdr plugin
link` writes exactly this; reproducing it here is what lets a store-built
plugin be registered without running an installer that would need to write
into the store.

Reads $pluginRoots (newline-separated), writes the registry to stdout.
"""

import json
import os
import tomllib


def entry(root):
    manifest = os.path.join(root, "herdr-plugin.toml")

    with open(manifest, "rb") as handle:
        entry = tomllib.load(handle)

    # The manifest calls it `id`; the registry calls it `plugin_id`.
    entry["plugin_id"] = entry.pop("id")
    entry["manifest_path"] = manifest
    entry["plugin_root"] = root
    entry["enabled"] = True

    # "local" is the linked-from-a-path source. The alternative, "github",
    # carries the checkout herdr manages itself and would fail its own
    # validation here: a store path is not inside herdr's managed checkout.
    entry["source"] = {"kind": "local"}
    return entry


roots = [root for root in os.environ["pluginRoots"].splitlines() if root]
print(json.dumps([entry(root) for root in roots], indent=2))
