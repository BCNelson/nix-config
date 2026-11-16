{ platform, inputs, pkgs, lib, ... }:
let
  # Rendered into /etc/nix/nix.custom.conf (see below). It is included in the
  # middle of Determinate's nix.conf, so a plain key here replaces the value
  # Determinate set above the include, and the extra- forms add to it.
  # experimental-features is left out: Determinate already enables
  # nix-command and flakes.
  customSettings = {
    auto-optimise-store = true;
    keep-outputs = true;
    keep-derivations = true;
    warn-dirty = false;

    # No channels are registered on system-manager hosts, so pin <nixpkgs>
    # to the same input the system is built from. This makes old-style
    # `nix-shell -p foo` / `nix-shell '<nixpkgs>'` resolve instead of
    # erroring with "file 'nixpkgs' was not found in the Nix search path".
    # It also replaces Determinate's extra-nix-path (FlakeHub nixpkgs-weekly).
    nix-path = [ "nixpkgs=${inputs.nixpkgs-unstable}" ];

    trusted-users = [ "root" "@wheel" "bcnelson" ];

    extra-substituters = [
      "https://devenv.cachix.org"
      "https://ai.cachix.org"
    ];
    extra-trusted-public-keys = [
      "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw="
      "ai.cachix.org-1:N9dzRK+alWwoKXQlnn0H6aUx0lU/mspIoz8hMvGvbbc="
    ];
  };

  customConf = lib.generators.toKeyValue {
    mkKeyValue = lib.generators.mkKeyValueDefault {
      mkValueString = v:
        if lib.isList v then lib.concatMapStringsSep " " toString v
        else lib.generators.mkValueStringDefault { } v;
    } " = ";
  } customSettings;
in
{
  imports = [
    ./_mixins/selinux.nix
    # Upstream candidate, kept verbatim so it can be lifted into a PR against
    # numtide/system-manager as nix/modules/sysusers.nix. See the README next
    # to it. Until that lands, import it here.
    ../contrib/system-manager-sysusers/module.nix
    ./_mixins/qmk.nix
    ./_mixins/uinput.nix
    ./_mixins/graphics.nix
    ./_mixins/agenix.nix
    ./_mixins/sysctl.nix
  ];

  config = {
    nixpkgs.hostPlatform = platform;

    environment.systemPackages = [ pkgs.powertop ];

    # Kept on for /etc/nix/registry.json and the nix client on PATH; the
    # nix.conf it would render is switched off below.
    nix.enable = true;

    # Determinate Nix owns /etc/nix/nix.conf. Whenever determinate-nixd starts,
    # or notices the file has changed, it moves whatever is there aside to
    # nix.conf.<timestamp>, writes its own config and restarts the daemon.
    # system-manager used to symlink its own nix.conf over the top on every
    # activation, so the two kept replacing each other. Most of the time the
    # daemon ran on Determinate's file, which has no trusted-users entry for
    # bcnelson. The result was "ignoring the client-specified setting
    # 'trusted-substituters' ... you are not a trusted user" while the nix.conf
    # on disk listed bcnelson.
    #
    # Determinate's file contains `!include nix.custom.conf` for local
    # settings, so ours go there instead. Turning off the nix.conf entry makes
    # system-manager restore the Determinate file it backed up when it first
    # took over (nix.conf.system-manager-backup).
    environment.etc."nix/nix.conf".enable = lib.mkForce false;
    environment.etc."nix/nix.custom.conf" = {
      replaceExisting = true;
      text = ''
        # Managed by system-manager -- see system-manager/default.nix
        ${customConf}
      '';
    };
  };
}
