#!/bin/sh
# macview, the Mac host's window on a Mac guest's display (gexc's
# GEX_FRAMEBUFFER_LIVE and GEX_HID_INPUT), built by this Mac's toolchain.
#
#   hosts/macview/build.sh OUT
set -eu
here=$(cd "$(dirname "$0")" && pwd)
xcrun clang -fobjc-arc -O2 -Wall -Wextra -Werror -framework Cocoa -framework QuartzCore -o "$1" "$here/macview.m"
