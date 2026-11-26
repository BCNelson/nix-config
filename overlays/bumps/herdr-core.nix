# herdr-core and herdr-desktop: pkgs/herdr-{core,desktop}/package.nix are
# copied from nixpkgs PR #592043 (core 0.1.22, desktop 0.1.16, head
# 0fb4b060), verbatim except herdr-core's tests are off (doCheck, marked
# there: the PR's suite needs /bin/bash and more the sandbox lacks). Once the channel ships that PR (or something later) these
# hand back to nixpkgs and its binary cache on their own; delete the vendored
# files then. The copy is whole files rather than an overrideAttrs bump
# because the PR also changes the postPatch substitutions and test skips that
# the new sources need.
#
# Why not wait: core 0.1.20 (the channel's) has its registry-writer
# thread eprintln! the save error, and eprintln! panics when stderr
# (core.log, on the same full disk) can't be written. The thread dies, every
# later tab open or label fails with "registry writer stopped" until the
# service restarts, and freeing space doesn't help. Fixed upstream in
# 0.1.22 (herdr-core d9e6826, a8ed0c3: saves now retry).
#
# callPackage resolves against final, so the desktop build is handed
# whichever herdr-core this picked, and the app launches that core.
{ final, ... }: final.callPackage ../../pkgs/herdr-core/package.nix { }
