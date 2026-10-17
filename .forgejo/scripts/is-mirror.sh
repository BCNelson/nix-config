#!/usr/bin/env bash
# Print "true" while this repo is still a pull mirror of GitHub, else "false".
# Steps that push or ping are gated on it: a mirror rejects pushes, and while
# it is one GitHub CI owns auto-update (and notify.yml sends the ping). Once
# the repo is converted to a regular repository, those steps start running
# with no further commit.
#
# Usage: is-mirror.sh <server_url> <owner/repo>
set -euo pipefail
curl -fsS "$1/api/v1/repos/$2" | grep -o '"mirror":[a-z]*' | head -1 | cut -d: -f2
