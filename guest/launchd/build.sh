#!/bin/sh
# gexlaunchd, for a Mac root under gex: built by this Mac's toolchain for
# macOS 10.14 and later, for each processor given (default both).
#
#   guest/launchd/build.sh OUTDIR [arm64|x86_64]...
set -eu
out=$1; shift
[ $# -gt 0 ] || set -- x86_64 arm64
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
for arch; do
  # (libxpc's pipe routines are its own interface, bound when it runs.)
  xcrun clang -arch "$arch" -mmacosx-version-min=10.14 -O2 -Wall -Wextra -Werror \
    -Wl,-undefined,dynamic_lookup -o "$out/gexlaunchd.$arch" "$here/gexlaunchd.c"
done
