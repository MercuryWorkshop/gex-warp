#!/bin/sh
# Copies a finished make wine / wine-x64 (wine/mac-build.sh) from the Docker
# volume to the host, and prints the host's WINE64_DIST= and WINE64_PAGE_PREFIX=:
#
#   wine/mac-export.sh DEST [x64|wow64]
#
# DEST/dist is the install with its links followed (the dist is a tree of
# links into the build), DEST/pfx the prefix made for the page. Not
# the raw prefix: its dosdevices/z: is a link to /, which a copy follows.
set -eu
IMAGE=${GEX_WINE_IMAGE:-gex-wine-build}
VOLUME=${VOLUME:-gex-wine-cache}
dest=${1:?DEST}; mkdir -p "$dest"; dest=$(cd "$dest" && pwd)
env=wine-x64.env; [ "${2:-x64}" != wow64 ] || env=wine.env
docker run --rm --platform linux/amd64 -v "$VOLUME":/root/.cache/gex -v "$dest":/dest -v "$(cd "$(dirname "$0")/.." && pwd)":/gex "$IMAGE" sh -c '
  . /root/.cache/gex/out/'$env'
  echo "export: $WINE64_DIST" >&2
  rm -rf "/dest/dist"; mkdir -p /dest/dist
  tar -C "$WINE64_DIST" -chf - . | tar -C /dest/dist -xf -
  # the prefix as the page takes it (page-prefix.sh: no dosdevices, real
  # files, the page user): GNU tar and the xattrs are in this image
  echo "export: $WINE64_PREFIX" >&2
  GRAPHICS=${GRAPHICS:-wayland} bash /gex/wine/page-prefix.sh "$WINE64_PREFIX" "$WINE64_DIST" /dest/pfx >&2
  # make wine-x64 puts the display driver (winewayland.drv) in no prefix (only
  # WoW64 builds do: dist.sh WD): without it in system32 the page finds no
  # driver ("no driver could be loaded"). The install'"'"'s is the same stub.
  cp -p "$WINE64_DIST/lib/wine/x86_64-windows/winewayland.drv" /dest/pfx/drive_c/windows/system32/'
echo "WINE64_DIST=$dest/dist"
echo "WINE64_PAGE_PREFIX=$dest/pfx"
