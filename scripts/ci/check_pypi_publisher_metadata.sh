#!/usr/bin/env bash
# Rehearse the actual publisher parser before any cross-registry mutation.
set -euo pipefail
test "$#" -eq 1
distribution_dir="$(realpath -- "$1")"
test -d "$distribution_dir"
publisher_tag=ghcr.io/pypa/gh-action-pypi-publish:dc37677b2e1c63e2034f94d8a5b11f265b73ba33
publisher_digest=sha256:a68d05519f6d7e47372aeaddab80b851b69afa89be179ec41775c72c4e3ab2d5
docker pull "$publisher_tag"
docker image inspect "$publisher_tag" --format '{{json .RepoDigests}}' |
  jq -e --arg expected "ghcr.io/pypa/gh-action-pypi-publish@$publisher_digest" \
    'index($expected) != null'
# No OIDC credentials, writable source, or network enter the parser rehearsal.
docker run --rm --network none --read-only \
  --mount "type=bind,source=$distribution_dir,target=/dist,readonly" \
  --entrypoint python \
  "ghcr.io/pypa/gh-action-pypi-publish@$publisher_digest" \
  -c 'import glob; from twine.commands.check import check; files=sorted(glob.glob("/dist/*")); raise SystemExit(int(not files or check(files, strict=True)))'
