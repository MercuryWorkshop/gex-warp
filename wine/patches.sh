#!/bin/sh
# gex's own changes to the pinned 64-bit Wine, which is a fork we do not push
# to: wine/patches/*.patch (git diffs or format-patch mails, in name order),
# applied over WINE64_COMMIT as one commit made the same way every time (fixed
# author, dates and message), so the same pin and patches give the same commit
# and wine/dist.sh's keys stay put. The pin itself is unchanged.
#
#   wine/patches.sh [--check] TREE PIN
#
# TREE is the pin's git worktree (scripts/fetch-pinned.sh --worktree). It ends
# at PIN with no patches, else at PIN plus the patch commit; nothing happens
# when it is already there (git touches only the files a change rewrites, so a
# rebuild is make's incremental one). --check only says whether it is (exit 0)
# or not (1), so the caller can leave a patched tree alone rather than move it
# back to PIN first.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
check=0; [ "${1:-}" = --check ] && { check=1; shift; }
tree=${1:?TREE} pin=${2:?PIN}
log() { echo "wine-patches: $*" >&2; }
set -- "$here"/patches/*.patch
[ -e "$1" ] || set --
key=$(for p in "$@"; do basename "$p"; cat "$p"; done | sha256sum | cut -c1-16)
mark="gex-patches: $key"
at() { # the tree is at PIN with the patches (or with none)
	[ -e "$tree/.git" ] || return 1
	head=$(git -C "$tree" rev-parse HEAD 2>/dev/null) || return 1
	[ -z "$(git -C "$tree" status --porcelain --untracked-files=no)" ] || return 1
	if [ $# -eq 0 ]; then [ "$head" = "$pin" ]; return; fi
	[ "$(git -C "$tree" rev-parse HEAD^ 2>/dev/null)" = "$pin" ] &&
		git -C "$tree" log -1 --format=%B HEAD | grep -qxF "$mark"
}
if at "$@"; then exit 0; fi
[ $check = 0 ] || exit 1
[ -z "$(git -C "$tree" status --porcelain --untracked-files=no)" ] || { log "$tree has uncommitted changes"; exit 1; }
git -C "$tree" checkout -q --detach "$pin"
[ $# -gt 0 ] || exit 0
for p in "$@"; do
	git -C "$tree" apply --index --whitespace=nowarn "$p" || { log "$(basename "$p") does not apply to $pin"; git -C "$tree" reset -q --hard "$pin"; exit 1; }
done
msg=$(printf 'gex: wine/patches\n\n%s\n\n%s\n' "$(for p in "$@"; do basename "$p"; done)" "$mark")
GIT_AUTHOR_NAME=gex GIT_AUTHOR_EMAIL=gex@localhost GIT_COMMITTER_NAME=gex GIT_COMMITTER_EMAIL=gex@localhost \
GIT_AUTHOR_DATE='1970-01-01T00:00:00Z' GIT_COMMITTER_DATE='1970-01-01T00:00:00Z' \
	git -C "$tree" -c commit.gpgsign=false commit -q --no-verify -m "$msg"
log "$tree: $(printf %.12s "$pin") + $# patch(es): $(git -C "$tree" rev-parse --short=12 HEAD)"
