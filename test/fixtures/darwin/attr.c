// getattrlist: a file's attributes asked for by bit and answered packed in
// bit order, the ones of no fixed size after the rest. And the "extended"
// calls, which carry an access list beside a mode: no file here has one.
#include "t.h"

enum { SYS_getattrlist = 220, SYS_fgetattrlist = 228, SYS_getattrlistat = 476, SYS_chmod_extended = 282, SYS_fchmod_extended = 283 };
enum { SYS_stat64_extended = 341, SYS_lstat64_extended = 342, SYS_fstat64_extended = 343 };
enum { SYS_setattrlist = 221, SYS_fsetattrlist = 229, SYS_setattrlistat = 524 };

struct attrlist {
    u16 bitmapcount, reserved;
    u32 common, volume, dir, file, fork;
};
enum {
    CMN_NAME = 1 << 0, CMN_DEVID = 1 << 1, CMN_FSID = 1 << 2, CMN_OBJTYPE = 1 << 3, CMN_OBJTAG = 1 << 4, CMN_OBJID = 1 << 5,
    CMN_PAROBJID = 1 << 7, CMN_SCRIPT = 1 << 8, CMN_MODTIME = 1 << 10, CMN_ACCTIME = 1 << 12, CMN_BKUPTIME = 1 << 13, CMN_FNDRINFO = 1 << 14,
    CMN_OWNERID = 1 << 15, CMN_GRPID = 1 << 16, CMN_ACCESSMASK = 1 << 17, CMN_FLAGS = 1 << 18, CMN_USERACCESS = 1 << 21,
    CMN_EXTENDED_SECURITY = 1 << 22, CMN_UUID = 1 << 23, CMN_FILEID = 1 << 25, CMN_PARENTID = 1 << 26, CMN_FULLPATH = 1 << 27,
    CMN_ERROR = 1 << 29, CMN_RETURNED_ATTRS = 1u << 31,
};
enum { VOL_FSTYPE = 1, VOL_MOUNTPOINT = 1 << 12, VOL_MOUNTFLAGS = 1 << 14, VOL_CAPABILITIES = 1 << 17, VOL_FSTYPENAME = 1 << 20, VOL_INFO = 1u << 31 };
enum { DIR_ENTRYCOUNT = 2, DIR_MOUNTSTATUS = 4 };
enum { FILE_LINKCOUNT = 1, FILE_TOTALSIZE = 2, FILE_DEVTYPE = 1 << 5, FILE_DATALENGTH = 1 << 9, FILE_RSRCLENGTH = 1 << 12 };
enum { NOFOLLOW = 1, REPORT_FULLSIZE = 4, CMN_EXTENDED = 0x20 };
enum { EXT_RELPATH = 4, EXT_LINKID = 0x10 };

static u8 buf[2048];

static long get(const char *path, u32 common, u32 volume, u32 dir, u32 file, u32 fork, long size, long options) {
    struct attrlist al = {5, 0, common, volume, dir, file, fork};
    for (unsigned i = 0; i < sizeof buf; i++) buf[i] = 0xee;
    return SYS(SYS_getattrlist, path, &al, buf, size, options);
}
static u32 w(long at) { return *(u32 *)(buf + at); }
static u64 q(long at) { return (u64)w(at) | (u64)w(at + 4) << 32; }
// The string an attrreference at `at` refers to, and its length.
static const char *ref(long at) { return (const char *)buf + at + (int)w(at); }
// The last component of a path.
static const char *leaf(const char *p) {
    const char *l = p;
    for (; *p; p++)
        if (*p == '/') l = p + 1;
    return l;
}

static void touch(const char *path) {
    long fd = SYS(SYS_open, path, O_WRONLY | O_CREAT, 0644);
    SYS(SYS_close, fd);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    struct t_stat st, dot;
    long fd = SYS(SYS_open, "a.txt", O_WRONLY | O_CREAT | O_TRUNC, 0640);
    SYS(SYS_write, fd, "hello, file\n", 12);
    SYS(SYS_close, fd);
    SYS(SYS_mkdir, "d", 0755);
    touch("d/one");
    touch("d/two");
    SYS(SYS_symlink, "a.txt", "l");
    SYS(SYS_stat64, "a.txt", &st);
    SYS(SYS_stat64, ".", &dot);

    // One attribute of each shape: a string, a number, a string again.
    t_show("name, type, mode, path", get("a.txt", CMN_NAME | CMN_OBJTYPE | CMN_ACCESSMASK | CMN_FULLPATH, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: name \"%s\" (%u) type %u mode %o path .../%s (", w(0), ref(4), w(8), w(12), w(16), leaf(ref(20)));
    t_check("its length", w(24) == t_strlen(ref(20)) + 1);
    t_check("  the name's data follows the fixed part", (int)w(4) == 28 - 4 && ref(20) == ref(4) + 8);
    t_check("  nothing is written past the length", buf[w(0)] == 0xee);

    // The numbers, which are what stat says.
    t_show("ids", get("a.txt", CMN_DEVID | CMN_OBJID | CMN_PAROBJID | CMN_OWNERID | CMN_GRPID | CMN_FILEID | CMN_PARENTID, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u\n", w(0));
    t_check("  device", w(4) == (u32)st.st_dev);
    t_check("  object id", w(8) == (u32)st.st_ino && w(12) == 0);
    t_check("  parent's", w(16) == (u32)dot.st_ino && w(20) == 0);
    t_check("  owner and group", w(24) == st.st_uid && w(28) == st.st_gid);
    t_check("  file id", q(32) == st.st_ino);
    t_check("  parent id", q(40) == dot.st_ino);

    t_show("what the volume says of it", get("a.txt", CMN_OBJTAG | CMN_SCRIPT | CMN_BKUPTIME | CMN_FNDRINFO | CMN_FLAGS | CMN_USERACCESS, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: tag %u script 0x%x backed up %lu.%lu flags %u access %u\n", w(0), w(4), w(8), (unsigned long)q(12), (unsigned long)q(20), w(60), w(64));
    int zero = 1;
    for (int i = 28; i < 60; i++) zero &= buf[i] == 0;
    t_check("  no Finder information", zero);
    t_show("its access list", get("a.txt", CMN_EXTENDED_SECURITY | CMN_ACCESSMASK, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: a list of %u bytes, at %d\n", w(0), w(12), (int)w(8));
    t_show("modified", get("a.txt", CMN_MODTIME, 0, 0, 0, 0, sizeof buf, 0));
    t_check("  when stat says", w(0) == 20 && q(4) == (u64)st.st_mtim.tv_sec && q(12) == (u64)st.st_mtim.tv_nsec);

    // A file's own, and a directory's; each is deaf to the other's.
    t_show("a file's", get("a.txt", CMN_OBJTYPE, 0, DIR_ENTRYCOUNT, FILE_LINKCOUNT | FILE_TOTALSIZE | FILE_DATALENGTH | FILE_RSRCLENGTH, 0, sizeof buf, 0));
    t_printf("  length %u: type %u links %u size %lu data %lu resource %lu\n", w(0), w(4), w(8), (unsigned long)q(12), (unsigned long)q(20), (unsigned long)q(28));
    t_show("a directory's", get("d", CMN_OBJTYPE | CMN_RETURNED_ATTRS, 0, DIR_ENTRYCOUNT | DIR_MOUNTSTATUS, FILE_TOTALSIZE, 0, sizeof buf, 0));
    t_printf("  length %u: returned 0x%x 0x%x 0x%x 0x%x 0x%x: type %u entries %u mounted %u\n", w(0), w(4), w(8), w(12), w(16), w(20), w(24), w(28), w(32));
    t_show("a file's, told what was returned", get("a.txt", CMN_NAME | CMN_RETURNED_ATTRS, 0, DIR_ENTRYCOUNT, FILE_TOTALSIZE, 0, sizeof buf, 0));
    t_printf("  length %u: returned 0x%x 0x%x 0x%x 0x%x 0x%x: \"%s\" size %lu\n", w(0), w(4), w(8), w(12), w(16), w(20), ref(24), (unsigned long)q(32));

    // A link, and what it leads to.
    t_show("through a link", get("l", CMN_NAME | CMN_OBJTYPE | CMN_FULLPATH, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  \"%s\" type %u path .../%s\n", ref(4), w(12), leaf(ref(16)));
    t_show("the link itself", get("l", CMN_NAME | CMN_OBJTYPE | CMN_FULLPATH, 0, 0, 0, 0, sizeof buf, NOFOLLOW));
    t_printf("  \"%s\" type %u path .../%s\n", ref(4), w(12), leaf(ref(16)));

    // A buffer too small: what fits, and the length of that or of it all.
    t_show("into 12 bytes", get("a.txt", CMN_NAME | CMN_OBJTYPE | CMN_FULLPATH, 0, 0, 0, 0, 12, 0));
    t_printf("  length %u, then 0x%x 0x%x\n", w(0), buf[11], buf[12]);
    long whole = get("a.txt", CMN_NAME | CMN_OBJTYPE | CMN_FULLPATH, 0, 0, 0, 0, sizeof buf, 0) == 0 ? (long)w(0) : -1;
    t_show("into 12 bytes, asking how much there is", get("a.txt", CMN_NAME | CMN_OBJTYPE | CMN_FULLPATH, 0, 0, 0, 0, 12, REPORT_FULLSIZE));
    t_check("  all of it", w(0) == (u32)whole);

    // The fifth group, as more common attributes.
    t_show("extended common attributes", get("d/one", CMN_NAME, 0, 0, 0, EXT_RELPATH | EXT_LINKID, sizeof buf, CMN_EXTENDED));
    SYS(SYS_stat64, "d/one", &st);
    t_printf("  \"%s\" relative .../%s\n", ref(4), leaf(ref(12)));
    t_check("  link id", q(20) == st.st_ino);
    t_show("the fifth group without the option", get("d/one", CMN_NAME, 0, 0, 0, EXT_RELPATH, sizeof buf, 0));

    // By descriptor, and under one.
    struct attrlist al = {5, 0, CMN_NAME | CMN_OBJTYPE, 0, 0, FILE_TOTALSIZE, 0};
    fd = SYS(SYS_open, "a.txt", O_RDONLY);
    t_show("fgetattrlist", SYS(SYS_fgetattrlist, fd, &al, buf, sizeof buf, 0));
    t_printf("  \"%s\" type %u size %lu\n", ref(4), w(12), (unsigned long)q(16));
    SYS(SYS_close, fd);
    t_show("fgetattrlist of no descriptor", SYS(SYS_fgetattrlist, 99, &al, buf, sizeof buf, 0));
    fd = SYS(SYS_open, "d", O_RDONLY);
    t_show("getattrlistat", SYS(SYS_getattrlistat, fd, "two", &al, buf, sizeof buf, 0));
    t_printf("  \"%s\" type %u size %lu\n", ref(4), w(12), (unsigned long)q(16));
    t_show("getattrlistat, from here", SYS(SYS_getattrlistat, -2, "d/one", &al, buf, sizeof buf, 0));
    t_printf("  \"%s\"\n", ref(4));
    SYS(SYS_close, fd);

    // The volume.
    t_show("the volume", get("/", 0, VOL_INFO | VOL_FSTYPE | VOL_MOUNTPOINT | VOL_CAPABILITIES | VOL_FSTYPENAME, 0, 0, 0, sizeof buf, 0));
    t_printf("  mounted on \"%s\", a \"%s\"; links %u, big files %u, attribute lists %u\n", ref(8), ref(48), w(16) & 6, w(16) & 0x800, w(20) & 2);
    t_check("  its capabilities are all said to be known", (w(32) & w(16)) == w(16) && (w(36) & w(20)) == w(20));
    // (Which volume a file here is on is the Mac's business: its Data one.)
    t_show("the volume a file is on", get("a.txt", 0, VOL_INFO | VOL_MOUNTPOINT, 0, 0, 0, sizeof buf, 0));
    t_check("  is mounted somewhere", ref(4)[0] == '/');

    // What is refused.
    t_show("a missing file", get("missing", CMN_NAME, 0, 0, 0, 0, sizeof buf, 0));
    // An error is a directory entry's, read in bulk; of one file it is
    // none, and comes first.
    t_show("an error", get("a.txt", CMN_NAME | CMN_OBJTYPE | CMN_ERROR, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: error %u, \"%s\" type %u\n", w(0), w(4), ref(8), w(16));
    t_show("an error, and what was returned", get("a.txt", CMN_NAME | CMN_ERROR | CMN_RETURNED_ATTRS, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: returned 0x%x: \"%s\"\n", w(0), w(4), ref(24));
    t_show("a volume attribute, not said to be one", get("/", 0, VOL_FSTYPENAME, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u: \"%s\"\n", w(0), ref(4));
    t_show("a volume's and a file's at once", get("/", 0, VOL_INFO | VOL_FSTYPE, 0, FILE_TOTALSIZE, 0, sizeof buf, 0));
    t_show("a file attribute there is none of", get("a.txt", 0, 0, 0, 1 << 6, 0, sizeof buf, 0));
    al.bitmapcount = 4;
    t_show("a list of four groups", SYS(SYS_getattrlist, "a.txt", &al, buf, sizeof buf, 0));
    t_show("nothing asked for", get("a.txt", 0, 0, 0, 0, 0, sizeof buf, 0));
    t_printf("  length %u\n", w(0));

    // stat with the file's security beside it: there is none.
    u8 sec[128];
    u64 size = sizeof sec;
    t_show("stat64_extended", SYS(SYS_stat64_extended, "a.txt", &st, sec, &size));
    t_printf("  size %lu mode %o, security of %lu bytes\n", (unsigned long)st.st_size, st.st_mode, (unsigned long)size);
    t_printf("  magic 0x%x, a list of %d entries, flags 0x%x; owner", *(u32 *)sec, *(i32 *)(sec + 36), *(u32 *)(sec + 40));
    for (int i = 4; i < 36; i++) t_printf("%s%02x", i == 20 ? " group " : " ", sec[i]);
    t_printf("\n");
    size = sizeof sec;
    t_show("lstat64_extended of the link", SYS(SYS_lstat64_extended, "l", &st, sec, &size));
    t_printf("  mode %o, security of %lu bytes\n", st.st_mode & 0170000, (unsigned long)size);
    fd = SYS(SYS_open, "a.txt", O_RDONLY);
    size = sizeof sec;
    t_show("fstat64_extended", SYS(SYS_fstat64_extended, fd, &st, sec, &size));
    t_printf("  size %lu, security of %lu bytes\n", (unsigned long)st.st_size, (unsigned long)size);
    t_show("fstat64_extended, wanting none", SYS(SYS_fstat64_extended, fd, &st, 0, 0));
    t_show("stat64_extended of a missing file", SYS(SYS_stat64_extended, "missing", &st, sec, &size));
    // A mode set with no owner, group or list beside it (-101 is none of
    // the first two, -1 no mode).
    t_show("fchmod_extended", SYS(SYS_fchmod_extended, fd, -101, -101, 0600, 0));
    SYS(SYS_fstat64, fd, &st);
    t_printf("  mode %o\n", st.st_mode & 07777);
    t_show("chmod_extended", SYS(SYS_chmod_extended, "a.txt", -101, -101, 0644, 0));
    SYS(SYS_fstat64, fd, &st);
    t_printf("  mode %o\n", st.st_mode & 07777);
    t_show("chmod_extended, nothing to change", SYS(SYS_chmod_extended, "a.txt", -101, -101, -1, 0));
    SYS(SYS_fstat64, fd, &st);
    t_printf("  mode %o\n", st.st_mode & 07777);
    t_show("chmod_extended of a missing file", SYS(SYS_chmod_extended, "missing", -101, -101, 0600, 0));
    SYS(SYS_close, fd);

    // Attributes given: the times, in the order of their bits (modified
    // before read), and the mode. This is how the libc sets a file's times.
    struct {
        struct t_timespec modified, read;
        u32 mode;
    } given = {{1000000000, 0}, {1000000001, 500}, 0604};
    al = (struct attrlist){5, 0, CMN_MODTIME | CMN_ACCTIME | CMN_ACCESSMASK, 0, 0, 0, 0};
    t_show("setattrlist", SYS(SYS_setattrlist, "a.txt", &al, &given, sizeof given, 0));
    SYS(SYS_stat64, "a.txt", &st);
    t_printf("  modified %ld.%ld read %ld.%ld mode %o\n", st.st_mtim.tv_sec, st.st_mtim.tv_nsec, st.st_atim.tv_sec, st.st_atim.tv_nsec, st.st_mode & 07777);
    given.modified.tv_sec = 1100000000;
    al.common = CMN_MODTIME;
    fd = SYS(SYS_open, "a.txt", O_RDONLY);
    t_show("fsetattrlist", SYS(SYS_fsetattrlist, fd, &al, &given, 16, 0));
    SYS(SYS_close, fd);
    given.modified.tv_sec = 1200000000;
    t_show("setattrlistat, through the link", SYS(SYS_setattrlistat, -2, "l", &al, &given, 16, 0));
    SYS(SYS_stat64, "a.txt", &st);
    t_printf("  modified %ld, read %ld still\n", st.st_mtim.tv_sec, st.st_atim.tv_sec);
    t_show("setattrlist with too little given", SYS(SYS_setattrlist, "a.txt", &al, &given, 8, 0));
    al.common = CMN_NAME;
    t_show("setattrlist of a name", SYS(SYS_setattrlist, "a.txt", &al, &given, sizeof given, 0));
    al.common = CMN_MODTIME;
    t_show("setattrlist of a missing file", SYS(SYS_setattrlist, "missing", &al, &given, 16, 0));

    // A directory removed as rm removes one: "without fetching what is in
    // it" (AT_REMOVEDIR_DATALESS).
    SYS(SYS_unlink, "l"), SYS(SYS_unlink, "d/one");
    t_show("unlinkat a directory that has something in it", SYS(SYS_unlinkat, -2, "d", 0x100));
    SYS(SYS_unlink, "d/two");
    t_show("unlinkat it empty", SYS(SYS_unlinkat, -2, "d", 0x100));
    t_show("and it is gone", SYS(SYS_stat64, "d", &st));
    SYS(SYS_unlink, "a.txt");
    return 0;
}
