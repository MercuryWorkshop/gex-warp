#!/bin/sh
# gexloginwindow, for a Mac root under gex: built by this Mac's toolchain
# for macOS 10.14 and later, for each processor given (default both), and
# signed ad hoc with the entitlement WindowServer asks of loginwindow.
#
#   guest/loginwindow/build.sh OUTDIR [arm64|x86_64]...
#
# The program is OUTDIR/ARCH/loginwindow: WindowServer knows loginwindow
# by its name.
set -eu
out=$1; shift
[ $# -gt 0 ] || set -- x86_64 arm64
here=$(cd "$(dirname "$0")" && pwd)
sdk=$(xcrun --show-sdk-path)
for arch; do
  mkdir -p "$out/$arch"
  xcrun clang -arch "$arch" -mmacosx-version-min=10.14 -O2 -Wall -Wextra -Werror \
    -F"$sdk/System/Library/PrivateFrameworks" -framework SkyLight -framework ApplicationServices \
    -o "$out/$arch/loginwindow" "$here/gexloginwindow.c"
  codesign -f -s - --entitlements "$here/entitlements.plist" "$out/$arch/loginwindow" 2>/dev/null
done
