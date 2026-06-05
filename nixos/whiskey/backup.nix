{ config, pkgs, libx, ... }:
let
  cadencePostHook = slug: ''
    url="https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})"
    if [ "$exitStatus" -ne 0 ]; then url="$url/fail"; fi
    ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 "$url" || true
  '';
  basicBorgJob = { repo, paths, cadenceSlug }: {
    inherit repo paths;
    encryption.mode = "none";
    environment.BORG_RSH = "ssh -o 'StrictHostKeyChecking=no' -i ${config.age.secrets.borgbaseSshKey.path}";
    environment.BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK = "yes";
    extraCreateArgs = "--verbose --stats --checkpoint-interval 600";
    compression = "zstd,1";
    startAt = "*-*-* 0/6:00:00";
    postHook = cadencePostHook cadenceSlug;
  };
  borgReposSecrets = libx.getSecretWithDefault ./sensitive.nix "borgRepos" {
    level1 = "";
    level2 = "";
    level3 = "";
    level4 = "";
    level5 = "";
  };
in
{
  age.secrets.borgbaseSshKey.rekeyFile = ../../secrets/store/whiskey/borgbase_ssh_key.age;

  services.borgbackup.jobs = {
    level1 = basicBorgJob {
      repo = borgReposSecrets.level1;
      paths = "/data/level1";
      cadenceSlug = "borgbackup-whiskey-level1";
    };
    level2 = basicBorgJob {
      repo = borgReposSecrets.level2;
      paths = "/data/level2";
      cadenceSlug = "borgbackup-whiskey-level2";
    };
    level3 = basicBorgJob {
      repo = borgReposSecrets.level3;
      paths = "/data/level3";
      cadenceSlug = "borgbackup-whiskey-level3";
    };
    # level4 = basicBorgJob {
    #   repo = borgReposSecrets.level4;
    #   paths = "/mnt/vault/data/level4";
    # };
    # level5 = basicBorgJob {
    #   repo = borgReposSecrets.level5;
    #   paths = "/mnt/vault/data/level5";
    # };
  };
}
