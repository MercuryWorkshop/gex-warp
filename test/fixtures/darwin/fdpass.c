// How SCM_RIGHTS travels with a stream's bytes: with the first byte it was sent with,
// across partial reads, peeks, and two sends read as one.
#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <sys/stat.h>
#include <sys/mman.h>

static int send_with(int s, const char *data, size_t n, int *fds, int nfds) {
  char cbuf[CMSG_SPACE(sizeof(int) * 8)]; memset(cbuf, 0, sizeof cbuf);
  struct iovec iov = { (void *)data, n };
  struct msghdr m = { 0 }; m.msg_iov = &iov; m.msg_iovlen = 1;
  if (nfds) {
    m.msg_control = cbuf; m.msg_controllen = CMSG_SPACE(sizeof(int) * nfds);
    struct cmsghdr *c = CMSG_FIRSTHDR(&m);
    c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS; c->cmsg_len = CMSG_LEN(sizeof(int) * nfds);
    memcpy(CMSG_DATA(c), fds, sizeof(int) * nfds);
  }
  return sendmsg(s, &m, 0);
}
static void recv_show(const char *what, int s, size_t want, int flags, size_t cspace) {
  char buf[256], cbuf[256]; memset(cbuf, 0, sizeof cbuf);
  struct iovec iov = { buf, want };
  struct msghdr m = { 0 }; m.msg_iov = &iov; m.msg_iovlen = 1;
  m.msg_control = cspace ? cbuf : NULL; m.msg_controllen = cspace;
  ssize_t r = recvmsg(s, &m, flags);
  int nf = 0, kinds = 0; char first[64] = "";
  for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c)) {
    kinds++;
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
      int n = (c->cmsg_len - CMSG_LEN(0)) / sizeof(int), fds[8]; memcpy(fds, CMSG_DATA(c), n * sizeof(int));
      for (int i = 0; i < n; i++) { char t[8] = ""; pread(fds[i], t, 7, 0); if (!nf) snprintf(first, sizeof first, "%s", t); nf++; close(fds[i]); }
    }
  }
  printf("%s: %zd bytes '%.*s', %d fds (first holds '%s'), %d control messages, controllen %zu, flags%s%s\n", what, r, r > 0 ? (int)r : 0, buf, nf, first, kinds,
         (size_t)m.msg_controllen, m.msg_flags & MSG_CTRUNC ? " CTRUNC" : "", m.msg_flags & MSG_TRUNC ? " TRUNC" : "");
}
static int file_of(const char *text) {
  int fd = memfd_create("t", MFD_CLOEXEC | MFD_ALLOW_SEALING); write(fd, text, strlen(text)); return fd;
}
int main(void) {
  int sv[2];
  socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv);
  int a = file_of("alpha"), b = file_of("beta"), c = file_of("gamma");
  int two[2] = { a, b };
  printf("== one send of 20 bytes with two fds, read 16 then 4\n");
  send_with(sv[0], "0123456789abcdefWXYZ", 20, two, 2);
  recv_show("first 16", sv[1], 16, 0, 256);
  recv_show("last 4", sv[1], 4, 0, 256);
  printf("== peeked, then read\n");
  send_with(sv[0], "peekpeek", 8, &c, 1);
  recv_show("peek 4", sv[1], 4, MSG_PEEK, 256);
  recv_show("read 4", sv[1], 4, 0, 256);
  recv_show("read 4 more", sv[1], 4, 0, 256);
  printf("== two sends, each with an fd, one read asking for both\n");
  send_with(sv[0], "AAAA", 4, &a, 1);
  send_with(sv[0], "BBBB", 4, &b, 1);
  recv_show("read 8", sv[1], 8, 0, 256);
  recv_show("read 8 again", sv[1], 8, MSG_DONTWAIT, 256);
  printf("== a send with an fd after plain bytes, one read asking for all\n");
  send_with(sv[0], "plain", 5, NULL, 0);
  send_with(sv[0], "withfd", 6, &c, 1);
  recv_show("read 11", sv[1], 11, 0, 256);
  recv_show("read 11 again", sv[1], 11, MSG_DONTWAIT, 256);
  printf("== no room for the control message\n");
  send_with(sv[0], "noroom", 6, &a, 1);
  recv_show("read with no control buffer", sv[1], 6, 0, 0);
  send_with(sv[0], "small", 5, two, 2);
  recv_show("read with room for one fd", sv[1], 5, 0, CMSG_SPACE(sizeof(int)));
  printf("== read by read(2), then recvmsg\n");
  send_with(sv[0], "readme", 6, &b, 1);
  char t[8] = ""; ssize_t r = read(sv[1], t, 3); printf("read: %zd '%s'\n", r, t);
  recv_show("then recvmsg", sv[1], 3, 0, 256);
  printf("== seals over the socket\n");
  int m = memfd_create("s", MFD_CLOEXEC | MFD_ALLOW_SEALING); ftruncate(m, 4096);
  printf("add seals: %d, get: %#x\n", fcntl(m, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL), fcntl(m, F_GET_SEALS));
  send_with(sv[0], "x", 1, &m, 1);
  { char buf[4], cbuf[64]; struct iovec iov = { buf, 1 }; struct msghdr mh = { 0 }; mh.msg_iov = &iov; mh.msg_iovlen = 1; mh.msg_control = cbuf; mh.msg_controllen = sizeof cbuf;
    recvmsg(sv[1], &mh, MSG_CMSG_CLOEXEC); struct cmsghdr *cm = CMSG_FIRSTHDR(&mh); int fd; memcpy(&fd, CMSG_DATA(cm), sizeof fd);
    struct stat st; fstat(fd, &st);
    printf("received: seals %#x, size %ld, cloexec %d\n", fcntl(fd, F_GET_SEALS), (long)st.st_size, fcntl(fd, F_GETFD)); }
  return 0;
}
