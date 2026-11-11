#!/bin/sh
# Copies a finished make wine / wine-x64 (wine/mac-build.sh) from the Docker
# volume to the host, and prints the host's WINE64_DIST= and WINE64_PREFIX=:
#
#   wine/mac-export.sh DEST [x64|wow64]
#
# DEST must be a directory on a case-sensitive file system or hold the result
# as a tar: a dist is a tree of 100k files, so it is packed on the way out
# with the symlinks kept, and unpacked here (macOS: case collisions in the
# prefix are reported by tar).
set -eu
IMAGE=${GEX_WINE_IMAGE:-gex-wine-build}
VOLUME=${VOLUME:-gex-wine-cache}
dest=${1:?DEST}; mkdir -p "$dest"; dest=$(cd "$dest" && pwd)
env=wine-x64.env; [ "${2:-x64}" != wow64 ] || env=wine.env
docker run --rm --platform linux/amd64 -v "$VOLUME":/root/.cache/gex -v "$dest":/dest "$IMAGE" sh -c '
  . /root/.cache/gex/out/'$env'
  for v in "$WINE64_DIST" "$WINE64_PREFIX"; do
    echo "export: $v" >&2
    tar -C "$(dirname "$v")" -chf - "$(basename "$v")" | tar -C /dest -xf -
  done
  basename "$WINE64_DIST" > /dest/.dist; basename "$WINE64_PREFIX" > /dest/.prefix'
echo "WINE64_DIST=$dest/$(cat "$dest/.dist")"
echo "WINE64_PREFIX=$dest/$(cat "$dest/.prefix")"
