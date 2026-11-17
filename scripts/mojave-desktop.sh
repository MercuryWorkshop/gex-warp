#!/bin/sh
# Mojave's desktop under gexc on this Mac: its WindowServer as gexlaunchd's
# job (guest/launchd), gexloginwindow (guest/loginwindow) in loginwindow's
# place, the daemons and agents on demand, and an app as the user (501).
#
#   scripts/mojave-desktop.sh ROOT OUT [APP]
#
# ROOT is an extracted macOS 10.14 root (x86-64), never written: it is
# bound read-only, and every directory something writes is bound to a copy
# under OUT/env, made afresh each run. APP is the program gexloginwindow
# starts as the user (default Calculator's).
#
# What it writes, under OUT: run.err (gexc and the daemons' log), run.out
# (gexloginwindow's lines), fb.png (the framebuffer every two seconds),
# fb.live (the framebuffer as it changes, for hosts/macview), input (lines
# of input for the HID system: `move X Y`, `click X Y`, `press CODE`,
# `sleep MS`, as gexc's GEX_HID_INPUT takes them).
#
# Settings, from the environment:
#   GEXC   the x86 gexc (default the tree's target/x86/release/gexc)
#   SECS   how long it goes on after the app starts (default 600)
#   SEED   a directory of saved state to start from, if it has some (the
#          written directories by name: folders, home, ...), and SAVE=1
#          keeps this run's there when it ends: LaunchServices' databases,
#          trustd's caches, the home. Without one each run registers every
#          application again, which takes a quarter of an hour.
#   VIEW=1 open a window on the display (hosts/macview), its mouse and keys
#          the guest's. Nothing is shown otherwise.
#   GEX_DSMOS_KEY  the key the system's protected programs are encrypted
#          with, gex's to know and never this script's: with it gexlaunchd
#          offers their jobs, and the desktop's own programs are started
#          with the app -- the Dock, the menu bar's SystemUIServer and the
#          Finder. Without it they are left out, and say so in run.err.
set -eu
[ $# -ge 2 ] || { echo "usage: $0 ROOT OUT [APP]" >&2; exit 2; }
R=$(cd "$1" && pwd)
mkdir -p "$2"
O=$(cd "$2" && pwd)
APP=${3:-/Applications/Calculator.app/Contents/MacOS/Calculator}
T=$(cd "$(dirname "$0")/.." && pwd)
G=${GEXC:-$T/target/x86/release/gexc}
SECS=${SECS:-600}
[ -x "$G" ] || { echo "no gexc at $G (make gexc)" >&2; exit 1; }
[ -e "$R/usr/lib/dyld" ] || { echo "$R is not a Mac root" >&2; exit 1; }
export GEX_CACHE_MAX_BYTES=${GEX_CACHE_MAX_BYTES:-1073741824}
ulimit -f 2097152

sh "$T/guest/launchd/build.sh" "$O/gl" x86_64
sh "$T/guest/loginwindow/build.sh" "$O/lw" x86_64

# The written directories, by name: where each is in the root, and whether
# it starts as the root's own (copied) or empty.
E=$O/env
rm -rf "$E"
mkdir -p "$E/etc"
binds=""
dir() {
  if [ -n "${SEED:-}" ] && [ -d "$SEED/$1" ]; then cp -a "$SEED/$1" "$E/$1"
  elif [ "$3" = copy ] && [ -d "$R$2" ]; then cp -a "$R$2" "$E/$1"
  else mkdir -p "$E/$1"; fi
}
set -- \
  folders /private/var/folders empty  tmp /private/var/tmp empty  run /private/var/run empty \
  log /private/var/log empty  caches /Library/Caches empty  tokens /private/var/db/TokenCache empty \
  mds /private/var/db/mds empty  coreduet /private/var/db/CoreDuet empty \
  tcc "/Library/Application Support/com.apple.TCC" empty  keychains /Library/Keychains empty \
  crls /private/var/db/crls empty  home /Users/gex empty  prefs /Library/Preferences copy \
  root /private/var/root copy  syscaches /System/Library/Caches copy
names=""
while [ $# -gt 0 ]; do dir "$1" "$2" "$3"; names="$names $1"; shift 3; done
mkdir -p "$E/home/Library/Preferences" "$E/home/Library/Caches" "$E/home/Library/Application Support"
# The user, gex (501), in a passwd of the root's and the home its own.
cp "$R/etc/passwd" "$R/etc/master.passwd" "$E/etc/"
echo 'gex:*:501:20:gex:/Users/gex:/bin/sh' >> "$E/etc/passwd"
echo 'gex:*:501:20::0:0:gex:/Users/gex:/bin/sh' >> "$E/etc/master.passwd"
[ -n "${SEED:-}" ] && [ -d "$SEED/home" ] || "$G" run --uid 0 --sysroot "$R" --bind-ro "$R=/" --bind "$E/home=/Users/gex" "$R/usr/sbin/chown" -R 501:20 /Users/gex

# gexloginwindow's job, and with the key the desktop's programs at load.
mkdir -p "$O/jobs"
cat > "$O/jobs/com.gex.loginwindow.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.loginwindow</string>
<key>ProgramArguments</key><array><string>/gex-login/loginwindow</string><string>-u</string><string>501:20</string><string>-n</string><string>gex</string><string>--</string><string>$APP</string></array>
</dict></plist>
PL
desktop=""
[ -n "${GEX_DSMOS_KEY+set}" ] && desktop="-r com.apple.Dock.agent -r com.apple.SystemUIServer.agent -r com.apple.Finder"

: > "$O/input"
rm -f "$O/fb.live"
if [ "${VIEW:-}" = 1 ]; then
  sh "$T/hosts/macview/build.sh" "$O/macview"
  "$O/macview" "$O/fb.live" "$O/input" &
fi
# (Not the daemons that would write into the root itself: the network's
# configuration, the disks', the log's housekeeping.)
OS_ACTIVITY_DT_MODE=YES GEX_FRAMEBUFFER_PNG="$O/fb.png" GEX_FRAMEBUFFER_LIVE="$O/fb.live" GEX_HID_INPUT="$O/input" \
  taskpolicy -c utility "$G" run --uid 0 --sysroot "$R" --bind-ro "$R=/" \
  --bind "$E/etc/passwd=/etc/passwd" --bind "$E/etc/master.passwd=/etc/master.passwd" \
  --bind "$E/folders=/private/var/folders" --bind "$E/tmp=/private/var/tmp" --bind "$E/run=/private/var/run" \
  --bind "$E/log=/private/var/log" --bind "$E/caches=/Library/Caches" --bind "$E/tokens=/private/var/db/TokenCache" \
  --bind "$E/mds=/private/var/db/mds" --bind "$E/coreduet=/private/var/db/CoreDuet" \
  --bind "$E/tcc=/Library/Application Support/com.apple.TCC" --bind "$E/keychains=/Library/Keychains" \
  --bind "$E/crls=/private/var/db/crls" --bind "$E/home=/Users/gex" --bind "$E/prefs=/Library/Preferences" \
  --bind "$E/root=/private/var/root" --bind "$E/syscaches=/System/Library/Caches" \
  --bind "$O/gl=/gex" --bind "$O/lw/x86_64=/gex-login" --bind "$O/jobs=/gex-jobs" \
  --env OS_ACTIVITY_DT_MODE=YES \
  "$O/gl/gexlaunchd.x86_64" -v -x com.apple.configd -x com.apple.diskarbitrationd -x com.apple.aslmanager \
  -a 501:20 -j /System/Library/LaunchDaemons -j /System/Library/LaunchAgents -j /gex-jobs -r com.gex.loginwindow $desktop \
  -- /System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer -daemon > "$O/run.out" 2> "$O/run.err" &
run=$!
# It goes on until SECS after the app has started, or the app ends.
until grep -aq 'gexloginwindow: started' "$O/run.out" 2>/dev/null || ! kill -0 $run 2>/dev/null; do sleep 5; done
t=0
while [ $t -lt "$SECS" ] && kill -0 $run 2>/dev/null && ! grep -aq -E 'gexloginwindow: /.*: (signal|exit)' "$O/run.out"; do sleep 5; t=$((t + 5)); done
sleep 5
kill $run 2>/dev/null || true
wait $run || true
if [ "${SAVE:-}" = 1 ] && [ -n "${SEED:-}" ]; then
  rm -rf "$SEED.new"; mkdir -p "$SEED.new"
  for n in $names; do [ "$n" = run ] || [ "$n" = tmp ] || cp -a "$E/$n" "$SEED.new/$n"; done
  find "$SEED.new" -type s -delete
  rm -rf "$SEED"; mv "$SEED.new" "$SEED"
fi
grep -a '^gexloginwindow: ' "$O/run.out" | tail -3
grep -a -E '\) crashed|is a protected binary' "$O/run.err" | cut -c1-200 || true
echo "the framebuffer: $O/fb.png"
