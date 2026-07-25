{ runCommandLocal, checkpolicy, semodule-utils }:

# SELinux loadable policy module that defines `nix_store_t` and grants the
# minimum permissions systemd's init_t domain needs to read and exec files
# under /nix/store. Intended for SELinux-enforcing hosts (e.g. Fedora) running
# system-manager. Install once with `just bootstrap-selinux`.
runCommandLocal "nix-store-selinux" {
  nativeBuildInputs = [ checkpolicy semodule-utils ];
} ''
  cat > nix_store.te <<'EOF'
  module nix_store 1.0;

  require {
      type init_t;
      type unconfined_t;
      type useradd_t;
      type systemd_coredump_t;
      type systemd_tmpfiles_t;
      type pasta_t;
      type iptables_t;
      attribute file_type;
      class file { read execute execute_no_trans open getattr ioctl map entrypoint };
      class dir { search read open getattr };
      class lnk_file { read getattr };
  }

  type nix_store_t;
  typeattribute nix_store_t file_type;

  # init_t needs to read unit files and exec helper binaries straight from
  # /nix/store (no domain transition; i.e. the engine and its restarts).
  allow init_t nix_store_t:dir { search read open getattr };
  allow init_t nix_store_t:file { read execute execute_no_trans open getattr ioctl map };
  allow init_t nix_store_t:lnk_file { read getattr };

  # When a unit sets SELinuxContext=...:unconfined_t / :useradd_t and exec's a
  # /nix/store binary, that binary must be a valid `entrypoint` for the target
  # domain. unconfined_t already has broad file-type permissions, so just add
  # entrypoint. useradd_t is scoped, so grant the full nix_store_t access set.
  allow unconfined_t nix_store_t:file entrypoint;

  allow useradd_t nix_store_t:dir { search read open getattr };
  allow useradd_t nix_store_t:file { read execute execute_no_trans open getattr ioctl map entrypoint };
  allow useradd_t nix_store_t:lnk_file { read getattr };

  # systemd-coredump's (sd-parse-elf) helper reads and mmaps a crashed
  # binary straight from /nix/store to build its backtrace. Read-only: it
  # never execs the target, so no execute perms. Without this, coredumps of
  # Nix-installed programs are logged but lack a resolved stack trace.
  allow systemd_coredump_t nix_store_t:dir { search read open getattr };
  allow systemd_coredump_t nix_store_t:file { read open getattr ioctl map };
  allow systemd_coredump_t nix_store_t:lnk_file { read getattr };

  # pasta is the rootless-container (Podman) usermode-networking helper,
  # itself installed under /nix/store, so it traverses and reads the store.
  allow pasta_t nix_store_t:dir { search read open getattr };
  allow pasta_t nix_store_t:file { read execute execute_no_trans open getattr ioctl map };
  allow pasta_t nix_store_t:lnk_file { read getattr };

  # systemd-tmpfiles stats the /nix/store directory itself during tmpfiles
  # runs (getattr on the dir, no descent into it).
  allow systemd_tmpfiles_t nix_store_t:dir { search getattr };

  # netavark (Podman's network backend) execs `nft` from /nix/store on every
  # container network setup/teardown; the resulting iptables_t process
  # traverses and reads the store (e.g. libnftables expression modules). The
  # nft rule ops succeed regardless, but each fires a denial — very high volume
  # on a host that churns containers.
  allow iptables_t nix_store_t:dir { search read open getattr };
  allow iptables_t nix_store_t:file { read execute execute_no_trans open getattr ioctl map };
  allow iptables_t nix_store_t:lnk_file { read getattr };
  EOF
  cat > nix_store.fc <<'EOF'
  /nix/store(/.*)?    system_u:object_r:nix_store_t:s0
  EOF
  mkdir -p $out
  # -c 19 pins the module ABI version so the .pp loads on hosts whose
  # libsepol is older than the build host's (Fedora 42 caps at v22; nixpkgs
  # checkpolicy defaults to v24, which fails to load).
  checkmodule -c 19 -M -m -o nix_store.mod nix_store.te
  semodule_package -o $out/nix_store.pp -m nix_store.mod -f nix_store.fc
''
