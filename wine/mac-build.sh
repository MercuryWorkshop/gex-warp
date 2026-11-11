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
# Env: JOBS (default 3), CPUS (default 3), VOLUME, GEX_WINE_IMAGE, SEEDS: a directory of bare
# clones wine-wasm.git, musl-wasm.git and musl-wasm64.git, for the pinned
# sources when the forks are private (the container has no credentials):
# git clone --bare URL SEEDS/NAME.git, with gh's or your credentials.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
IMAGE=${GEX_WINE_IMAGE:-gex-wine-build}
VOLUME=${VOLUME:-gex-wine-cache}
case ${1:-x64} in x64) T=wine-x64; shift || true ;; wow64) T=wine; shift ;; *) T=wine-x64 ;; esac
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build --platform linux/amd64 -t "$IMAGE" -f "$here/Dockerfile" "$here"
# a git worktree's .git file points outside the tree: mount the common dir too
common=$(cd "$root" && git rev-parse --git-common-dir | xargs -I{} sh -c 'cd "{}" && pwd')
seed=
if [ -n "${SEEDS:-}" ]; then
  seed="-v $SEEDS:/seed -e WINE64_SEED=/seed/wine-wasm.git -e WINE32_SEED=/seed/wine-wasm.git -e MUSL_SEED=/seed/musl-wasm.git -e MUSL64_SEED=/seed/musl-wasm64.git"
fi
exec docker run --rm --cpus "${CPUS:-3}" --platform linux/amd64 -v "$root":/gex -v "$common":"$common" -v "$VOLUME":/root/.cache/gex $seed \
  -w /gex -e JOBS="${JOBS:-3}" -e WINE32_JOBS="${JOBS:-3}" -e HOME=/root "$IMAGE" nice -n 10 make "$T" JOBS="${JOBS:-3}" OUT=/root/.cache/gex/out "$@"
