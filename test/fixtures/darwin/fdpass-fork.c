// A child makes a memfd, fills it, and passes it to its parent over a socketpair
// (as an image loader passes a frame): what the parent gets.
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
#include <sys/wait.h>

static int send_fd(int s, const char *data, size_t n, int fd) {
  char cbuf[CMSG_SPACE(sizeof(int))]; memset(cbuf, 0, sizeof cbuf);
  struct iovec iov = { (void *)data, n };
  struct msghdr m = { 0 }; m.msg_iov = &iov; m.msg_iovlen = 1;
  m.msg_control = cbuf; m.msg_controllen = sizeof cbuf;
  struct cmsghdr *c = CMSG_FIRSTHDR(&m);
  c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS; c->cmsg_len = CMSG_LEN(sizeof(int));
  memcpy(CMSG_DATA(c), &fd, sizeof fd);
  return sendmsg(s, &m, MSG_NOSIGNAL);
}
static int recv_fd(int s, char *buf, size_t want, ssize_t *got, int *flags) {
  char cbuf[CMSG_SPACE(sizeof(int) * 16)]; memset(cbuf, 0, sizeof cbuf);
  struct iovec iov = { buf, want };
  struct msghdr m = { 0 }; m.msg_iov = &iov; m.msg_iovlen = 1; m.msg_control = cbuf; m.msg_controllen = sizeof cbuf;
  *got = recvmsg(s, &m, MSG_CMSG_CLOEXEC); *flags = m.msg_flags;
  for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c))
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) { int fd; memcpy(&fd, CMSG_DATA(c), sizeof fd); return fd; }
  return -1;
}
int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  int sv[2]; socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv);
  pid_t pid = fork();
  if (pid == 0) {
    close(sv[0]);
    // 1: a memfd written with write()
    int a = memfd_create("written", MFD_CLOEXEC | MFD_ALLOW_SEALING); write(a, "written by the child", 20);
    send_fd(sv[1], "A", 1, a); close(a);
    // 2: a memfd sized, mapped and filled through the mapping, as a frame is
    int b = memfd_create("mapped", MFD_CLOEXEC | MFD_ALLOW_SEALING); ftruncate(b, 65536);
    char *p = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, b, 0);
    memset(p, 'm', 65536); memcpy(p, "mapped by the child", 19); memcpy(p + 65536 - 4, "END!", 4);
    munmap(p, 65536);
    fcntl(b, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW);
    send_fd(sv[1], "B", 1, b); close(b);
    // 3: the same, still mapped here when sent, and written after the send
    int c = memfd_create("live", MFD_CLOEXEC); ftruncate(c, 4096);
    char *q = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, c, 0);
    strcpy(q, "before the send");
    send_fd(sv[1], "C", 1, c);
    char ack; read(sv[1], &ack, 1);
    strcpy(q, "after the send");
    write(sv[1], "D", 1);
    read(sv[1], &ack, 1);
    // 4: a pipe's and a file's descriptor
    int pp[2]; pipe(pp); write(pp[1], "through a pipe", 14);
    send_fd(sv[1], "E", 1, pp[0]);
    int f = open("/proc/self/exe", O_RDONLY); send_fd(sv[1], "F", 1, f);
    _exit(0);
  }
  close(sv[1]);
  char tag[4], text[64]; ssize_t got; int flags, fd; struct stat st;
  fd = recv_fd(sv[0], tag, 1, &got, &flags);
  memset(text, 0, sizeof text); ssize_t r = fd < 0 ? -1 : pread(fd, text, 63, 0); fstat(fd, &st);
  printf("written: %zd byte '%.1s', fd %s, flags %#x, size %ld, read %zd '%s', seals %#x\n", got, tag, fd < 0 ? "none" : "there", flags, fd < 0 ? -1L : (long)st.st_size, r, text, fd < 0 ? -1 : fcntl(fd, F_GET_SEALS));
  fd = recv_fd(sv[0], tag, 1, &got, &flags);
  memset(text, 0, sizeof text); r = fd < 0 ? -1 : pread(fd, text, 19, 0); if (fd >= 0) fstat(fd, &st);
  char *p = fd < 0 ? MAP_FAILED : mmap(NULL, 65536, PROT_READ, MAP_SHARED, fd, 0);
  printf("mapped: %zd byte '%.1s', fd %s, size %ld, read %zd '%s', seals %#x, mapped %s '%.19s' ... '%.4s'\n", got, tag, fd < 0 ? "none" : "there", fd < 0 ? -1L : (long)st.st_size, r, text,
         fd < 0 ? -1 : fcntl(fd, F_GET_SEALS), p == MAP_FAILED ? "no" : "yes", p == MAP_FAILED ? "" : p, p == MAP_FAILED ? "" : p + 65536 - 4);
  fd = recv_fd(sv[0], tag, 1, &got, &flags);
  char *q = fd < 0 ? MAP_FAILED : mmap(NULL, 4096, PROT_READ, MAP_SHARED, fd, 0);
  printf("live: fd %s, mapped '%s'\n", fd < 0 ? "none" : "there", q == MAP_FAILED ? "" : q);
  write(sv[0], "k", 1); read(sv[0], tag, 1);
  memset(text, 0, sizeof text); if (fd >= 0) pread(fd, text, 63, 0);
  printf("live, after the child wrote again: mapped '%s', read '%s'\n", q == MAP_FAILED ? "" : q, text);
  write(sv[0], "k", 1);
  fd = recv_fd(sv[0], tag, 1, &got, &flags);
  memset(text, 0, sizeof text); r = fd < 0 ? -1 : read(fd, text, 63);
  printf("pipe: fd %s, read %zd '%s'\n", fd < 0 ? "none" : "there", r, text);
  fd = recv_fd(sv[0], tag, 1, &got, &flags);
  memset(text, 0, sizeof text); r = fd < 0 ? -1 : read(fd, text, 4);
  printf("file: fd %s, read %zd, ELF %d\n", fd < 0 ? "none" : "there", r, !memcmp(text, "\177ELF", 4));
  int status; waitpid(pid, &status, 0); printf("child: %d\n", status);
  return 0;
}
