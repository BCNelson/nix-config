{
  config,
  inputs,
  ...
}:
# graphene-nix: builds re-signed GrapheneOS OTAs (official release + our
# additions in `product`, signed with our avb/ota keys) whenever GrapheneOS
# ships a release, and serves them to the phones' Updater at
# https://ota.phone.bcnelson.dev/. Build and serve both live on romeo; the
# modules come from the graphene-nix flake (README there has the full story).
#
# Keys: the four private ones are hand-imported agenix secrets
# (scripts/graphene-import-keys <dir>, then `just rekey`), rekeyed for romeo
# only; the module hands each to the service with LoadCredential, which
# systemd reads as root. The public certs are plain files in ./graphene-nix.
# avb_pkmd.bin there is not used on romeo (the builder derives it from
# avb.key); it is what spike/flash-initial.sh needs on the workstation.
#
# avb.key is the one key that matters: a phone locked with it accepts updates
# only signed by it, so losing it means wiping phones to move them. The master
# copy is the agenix store in this repo.
#
# Changing what goes into the image -- the URL, `probes`, the graphene-nix
# scripts -- makes the next run respin the current release (N.1, N.2, ...), so
# phones get it without waiting for GrapheneOS. Moving the URL means keeping
# the old one serving until every phone has installed a release that points at
# the new one.
let
  dataDirs = config.data.dirs;
  host = "ota.phone.bcnelson.dev";
  secrets = config.age.secrets;
in {
  imports = [inputs.graphene-nix.nixosModules.default];

  age.secrets = {
    graphene-nix-avb-key.rekeyFile = ./secrets/graphene-nix/avb_key.age;
    graphene-nix-ota-key.rekeyFile = ./secrets/graphene-nix/ota_key.age;
    graphene-nix-probe-pk8.rekeyFile = ./secrets/graphene-nix/probe_pk8.age;
    graphene-nix-spike-pass.rekeyFile = ./secrets/graphene-nix/spike_pass.age;
  };

  services.graphene-nix = {
    builder = {
      enable = true;
      devices = ["caiman"];
      # updaterUrl defaults to https://${host}/ from the server below.

      keys = {
        avb = secrets.graphene-nix-avb-key.path;
        ota = secrets.graphene-nix-ota-key.path;
        apk = secrets.graphene-nix-probe-pk8.path;
        passphrase = secrets.graphene-nix-spike-pass.path;
        otaCert = ./graphene-nix/ota.crt;
        apkCert = ./graphene-nix/probe.crt.pem;
      };

      # No test app / settings overlay / permission grant on real phones: only
      # the Updater overlay goes into product.
      probes = false;

      # outDir only holds rebuildable output: losing it costs the next
      # release's incrementals (phones download the full OTA instead), so it
      # lives on replaceable storage. ~1.7 GB per kept full OTA per device.
      # The module makes the builder wait for its mount.
      outDir = "${dataDirs.level6}/graphene-nix/ota";
      # ~10 GB + ~5 GB per incremental while building, emptied afterwards. The
      # root NVMe; the default under /var/cache would be the same disk anyway.
      workDir = "${dataDirs.level7}/graphene-nix";
      deltas = 2;
    };

    server = {
      enable = true;
      hostName = host;
    };
  };

  # The module's RequiresMountsFor on outDir orders against nothing here:
  # trove/replaceable is a ZFS-native mount, whose .mount unit systemd only
  # tracks passively. Without this, a build started at boot (buildOnChange)
  # can beat zfs-mount.service and write its release into the bare mountpoint
  # on the root disk, where it is then hidden under the dataset.
  systemd.services.graphene-nix-builder = {
    requires = ["zfs-mount.service"];
    after = ["zfs-mount.service"];
  };

  ##########################################################################
  # TLS / nginx
  #
  # bcnelson.dev is a Cloudflare zone, so this cert uses Cloudflare DNS-01
  # instead of romeo's Porkbun default. The env file (and the token secret
  # behind it) is declared in ./trilium.nix, which does the same for
  # notes.bcnelson.dev.
  ##########################################################################
  security.acme.certs.${host} = {
    dnsProvider = "cloudflare";
    environmentFile = config.age-template.files."bcnelson-dev-cloudflare-acme-env".path;
  };

  services.nginx.virtualHosts.${host} = {
    acmeRoot = null;
    http2 = true;
  };
}
