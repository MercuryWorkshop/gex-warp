#!/usr/bin/env python3
"""Pack a root filesystem directory into a tar whose entries carry the owners
the root means them to have, for the page runtime's `addTar(src, { owners:
true })`.

A Mac root built by an ordinary user (pyrus's out/root, out/root-x86_64) is
all that user's on disk: `tar` would name uid 501 for every file, and a page
that believed it would hand the whole system to whoever has that uid in the
guest. pyrus records what each file is meant to be instead, as the extended
attribute `user.gex.owner` ("UID:GID": the set-user-ID su, sudo, login and
passwd are root's, /Users is root:admin, and so on), which is what gex-warp's
kernel reads natively (crates/gex-kernel/src/vfs/hostfs.rs). Here each entry
is that owner where the attribute is there, and root:wheel (0:0) otherwise,
which is what a Mac's system files are. Modes (set-id bits included), times,
symbolic and hard links are kept; the other `user.` attributes travel as pax
`SCHILY.xattr.user.*` records (the page keeps them); the owner attribute
itself, and the Mac's own (`com.apple.*`), are left out. No user or group
names are written, only numbers (the packer's own name is nobody's business
in the guest).

    scripts/pack-root.py [--exclude GLOB]... OUT.tar DIR [DIR...]

Each DIR's contents go at the archive's top, the later over the earlier, as
`tar -cf OUT -C DIR1 . -C DIR2 .` would lay them. `--exclude` takes a glob of
paths as the archive names them (`./private/var/run/pyrus/apps/*`).
"""

import argparse
import ctypes
import ctypes.util
import fnmatch
import os
import sys
import tarfile

OWNER_XATTR = 'user.gex.owner'
XATTR_NOFOLLOW = 0x0001

_libc = ctypes.CDLL(ctypes.util.find_library('c'), use_errno=True)


def _xattr_names(path):
    """The extended attributes' names of `path` itself (links not followed)."""
    if not hasattr(_libc, 'listxattr'):
        return []
    p = os.fsencode(path)
    n = _libc.listxattr(p, None, 0, XATTR_NOFOLLOW)
    if n <= 0:
        return []
    buf = ctypes.create_string_buffer(n)
    n = _libc.listxattr(p, buf, n, XATTR_NOFOLLOW)
    if n <= 0:
        return []
    return [x.decode('utf-8', 'surrogateescape') for x in buf.raw[:n].split(b'\0') if x]


def _xattr(path, name):
    p = os.fsencode(path)
    nm = name.encode('utf-8', 'surrogateescape')
    n = _libc.getxattr(p, nm, None, 0, 0, XATTR_NOFOLLOW)
    if n < 0:
        return None
    buf = ctypes.create_string_buffer(max(n, 1))
    n = _libc.getxattr(p, nm, buf, n, 0, XATTR_NOFOLLOW)
    return None if n < 0 else buf.raw[:n]


def _owner(path):
    """The owner `user.gex.owner` names, or root:wheel."""
    v = _xattr(path, OWNER_XATTR)
    if v is None:
        return 0, 0
    try:
        u, g = v.decode().strip().split(':')
        return int(u), int(g)
    except ValueError:
        raise SystemExit(f'pack-root: {path}: {OWNER_XATTR} is {v!r}, not UID:GID')


def pack(out, dirs, excludes):
    seen_top = False
    with tarfile.open(out, 'w', format=tarfile.PAX_FORMAT) as tar:
        for top in dirs:
            top = os.path.abspath(top)
            if not os.path.isdir(top):
                raise SystemExit(f'pack-root: {top}: not a directory')
            def arc_of(path):
                r = os.path.relpath(path, top)
                return '.' if r == '.' else './' + r

            def excluded(arc):
                return any(fnmatch.fnmatch(arc, x) or fnmatch.fnmatch(arc + '/', x) for x in excludes)

            for here, subdirs, files in os.walk(top):
                if excluded(arc_of(here)):
                    subdirs[:] = []
                    continue
                subdirs.sort()
                names = [here] + [os.path.join(here, f) for f in sorted(files)]
                # (A link to a directory is an entry of its own: os.walk
                # lists it among `subdirs`, and is not to go into it.)
                links = [d for d in subdirs if os.path.islink(os.path.join(here, d))]
                names += [os.path.join(here, d) for d in links]
                subdirs[:] = [d for d in subdirs if d not in links]
                for path in names:
                    arc = arc_of(path)
                    if arc == '.':
                        if seen_top:
                            continue
                        seen_top = True
                    if path != here and excluded(arc):
                        continue
                    ti = tar.gettarinfo(path, arcname=arc)
                    if ti is None:  # a socket: no tar entry for it
                        continue
                    ti.uid, ti.gid = _owner(path)
                    ti.uname = ti.gname = ''
                    for name in _xattr_names(path):
                        if name.startswith('user.') and name != OWNER_XATTR:
                            v = _xattr(path, name)
                            if v is not None:
                                ti.pax_headers['SCHILY.xattr.' + name] = v.decode('utf-8', 'surrogateescape')
                    if ti.isreg():
                        with open(path, 'rb') as f:
                            tar.addfile(ti, f)
                    else:
                        tar.addfile(ti)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--exclude', action='append', default=[], metavar='GLOB')
    ap.add_argument('out')
    ap.add_argument('dirs', nargs='+')
    a = ap.parse_args()
    tmp = a.out + '.part'
    try:
        pack(tmp, a.dirs, a.exclude)
        os.replace(tmp, a.out)
    except BaseException:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise


if __name__ == '__main__':
    sys.exit(main())
