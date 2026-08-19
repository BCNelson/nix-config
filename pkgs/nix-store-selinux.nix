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
      type sshd_key_t;
      type tmpfs_t;
      type etc_t;
      type fs_t;
      type devlog_t;
      type syslogd_t;
      type null_device_t;
      type urandom_device_t;
      type random_device_t;
      type device_t;
      type proc_t;
      type syslogd_var_run_t;
      type cgroup_t;
      type init_var_run_t;
      type passwd_file_t;
      attribute file_type;
      attribute domain;
      role system_r;
      class file { read write create open getattr setattr ioctl map lock append rename unlink link execute execute_no_trans entrypoint };
      class dir { search read write open getattr setattr add_name remove_name rmdir create };
      class lnk_file { read getattr create unlink };
      class fifo_file { read write open getattr create unlink ioctl lock };
      class chr_file { read write open getattr ioctl append };
      class process { fork sigchld signal getsched setpgid transition };
      class process2 { nnp_transition nosuid_transition };
      class capability { chown fowner dac_read_search dac_override };
      class unix_stream_socket { create connect write read getattr ioctl };
      class unix_dgram_socket { create connect write sendto };
      class sock_file { write getattr };
      class fd { use };
      class filesystem { getattr };
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

  # agenix's secret-decryption service. Its SELinuxContext= (set in
  # system-manager/_mixins/selinux.nix) previously named unconfined_t purely so
  # it could read the SSH host key, which handed a service that reads one file
  # and writes a handful the run of the whole machine. agenix_t is scoped to
  # exactly that job.
  #
  # No type_transition: the binary lives in /nix/store and so is nix_store_t,
  # which every other system-manager service also execs. A transition keyed on
  # that type would drag all of them into this domain. The unit names the domain
  # explicitly instead (setexeccon), which needs `entrypoint` -- the same
  # arrangement useradd_t above uses.
  type agenix_t;
  typeattribute agenix_t domain;
  # Without this the type exists but system_u:system_r:agenix_t:s0 is not a
  # valid context, so systemd's setexeccon fails with EPERM
  # (status=229/SELINUX_CONTEXT) and -- because the kernel rejects the context
  # before any permission check -- logs no AVC at all. refpolicy's
  # init_daemon_domain() macro would add this; raw .te has to say it.
  role system_r types agenix_t;

  # Secrets get their own type rather than inheriting tmpfs_t, so "can read a
  # decrypted agenix secret" is expressible in policy instead of collapsing into
  # "can read any tmpfs file". file_type gives unconfined_t (the domain a
  # desktop session runs in, and so goose-desktop) its normal read access.
  type agenix_secret_t;
  typeattribute agenix_secret_t file_type;
  type_transition agenix_t tmpfs_t:file agenix_secret_t;
  type_transition agenix_t tmpfs_t:dir agenix_secret_t;

  allow init_t agenix_t:process { transition };
  # Several of the sandbox options on the unit imply NoNewPrivileges=yes, and
  # the kernel refuses an SELinux domain transition under no_new_privs unless
  # the policy says otherwise. Harmless if nothing sets it.
  allow init_t agenix_t:process2 { nnp_transition nosuid_transition };

  # Run from /nix/store.
  allow agenix_t nix_store_t:dir { search read open getattr };
  allow agenix_t nix_store_t:file { read execute execute_no_trans open getattr ioctl map entrypoint };
  allow agenix_t nix_store_t:lnk_file { read getattr };

  # The whole point: read the host key used as the age identity. Read-only, and
  # only this type -- it cannot touch anything else under /etc.
  allow agenix_t sshd_key_t:file { read open getattr };
  allow agenix_t etc_t:dir { search getattr open read };
  allow agenix_t etc_t:lnk_file { read getattr };

  # Own its tmpfs and the secrets on it.
  allow agenix_t tmpfs_t:dir { search read write open getattr setattr add_name remove_name };
  allow agenix_t tmpfs_t:filesystem { getattr };
  allow agenix_t agenix_secret_t:dir { search read write open getattr setattr add_name remove_name create rmdir };
  allow agenix_t agenix_secret_t:file { create open read write getattr setattr rename unlink link append lock ioctl map };
  allow agenix_t agenix_secret_t:lnk_file { read getattr create unlink };

  # chown the secret to its owner; read a 0600 root-owned key.
  allow agenix_t self:capability { chown fowner dac_read_search };

  # Ordinary shell plumbing: pipes for $(...), fds inherited from systemd,
  # journal logging over /dev/log, /dev/null.
  allow agenix_t self:process { fork sigchld signal getsched setpgid };
  allow agenix_t self:fifo_file { read write open getattr create unlink ioctl lock };
  allow agenix_t self:unix_stream_socket { create connect write read getattr };
  allow agenix_t self:unix_dgram_socket { create connect write };
  allow agenix_t init_t:fd { use };
  allow agenix_t devlog_t:lnk_file { read getattr };
  allow agenix_t devlog_t:sock_file { write getattr };
  allow agenix_t syslogd_t:unix_dgram_socket { sendto };
  # stdout/stderr are a socket to journald, opened by systemd and inherited
  # across the exec, so the fd is journald's object rather than one this domain
  # created. Without this the service is silent in the journal even when it
  # runs, which is the worst way to debug the next problem.
  allow agenix_t syslogd_t:unix_stream_socket { read write getattr };
  allow agenix_t syslogd_var_run_t:sock_file { write getattr };
  allow agenix_t null_device_t:chr_file { read write open getattr ioctl append };
  allow agenix_t urandom_device_t:chr_file { read open getattr ioctl };
  allow agenix_t random_device_t:chr_file { read open getattr ioctl };
  allow agenix_t device_t:dir { search getattr };
  allow agenix_t proc_t:dir { search getattr };
  allow agenix_t proc_t:file { read open getattr };
  # Measured, not guessed: these are what `ausearch -m AVC | audit2allow`
  # reported after one permissive run on redo-3.
  #
  # passwd_file_t is the interesting one -- `chown bcnelson:bcnelson` has to
  # resolve the name through /etc/passwd, and without it the chown fails, the
  # script aborts under `set -e`, and a half-installed secret is left behind.
  allow agenix_t passwd_file_t:file { getattr open read };
  # stdout/stderr are a socket systemd created and passed across the exec, so
  # the object is labelled init_t rather than journald's own type.
  allow agenix_t init_t:unix_stream_socket { getattr ioctl };
  allow agenix_t init_var_run_t:dir search;
  allow agenix_t init_var_run_t:lnk_file read;
  allow agenix_t cgroup_t:dir search;

  # /proc/self entries carry the reading process's own context.
  allow agenix_t self:dir { search read open getattr };
  allow agenix_t self:file { read open getattr };
  allow agenix_t self:lnk_file { read getattr };

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
