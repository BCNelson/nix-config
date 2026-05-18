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
