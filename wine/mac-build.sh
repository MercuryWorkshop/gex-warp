#!/bin/sh
# make wine / make wine-x64 on an arm64 or x86-64 Mac, in the Linux x86-64
# container wine/Dockerfile describes (Docker or OrbStack). The tree is
# bind-mounted; the cache (sources, sysroots, every dist step) and OUT live in
# the Docker volume VOLUME (default gex-wine-cache), so reruns are incremental.
# Not a bind mount of ~/.cache/gex: the host's file system is case-insensitive
# and slow for a build tree, and wineboot needs a real Unix one.
#
#   wine/mac-build.sh [x64|wow64] [make args]     default x64 (make wine-x64)
#
# The printed WINE64_* paths are the container's (/root/.cache/gex/...).
# wine/mac-export.sh copies a finished dist and prefix to the host.
# Env: JOBS (default 6), VOLUME, GEX_WINE_IMAGE.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
IMAGE=${GEX_WINE_IMAGE:-gex-wine-build}
VOLUME=${VOLUME:-gex-wine-cache}
case ${1:-x64} in x64) T=wine-x64; shift || true ;; wow64) T=wine; shift ;; *) T=wine-x64 ;; esac
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build --platform linux/amd64 -t "$IMAGE" -f "$here/Dockerfile" "$here"
# a git worktree's .git file points outside the tree: mount the common dir too
common=$(cd "$root" && git rev-parse --git-common-dir | xargs -I{} sh -c 'cd "{}" && pwd')
exec docker run --rm --platform linux/amd64 -v "$root":/gex -v "$common":"$common" -v "$VOLUME":/root/.cache/gex \
  -w /gex -e JOBS="${JOBS:-6}" -e HOME=/root "$IMAGE" nice -n 10 make "$T" JOBS="${JOBS:-6}" OUT=/root/.cache/gex/out "$@"
