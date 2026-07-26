{
  # PLACEHOLDER — replace after installing delta-1.
  #
  # This is not the machine's key. agenix will happily rekey secrets to it and
  # the host will then be unable to decrypt any of them. After the first boot,
  # run `ssh-keyscan -t ed25519 delta-1` (or read
  # /etc/ssh/ssh_host_ed25519_key.pub on the device), paste the result here,
  # and run `just rekey`.
  hostKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYxPB/srptmJqQmW6MPb56MOv6F7RvY86fIy4ufylwj";
}
