// The Darwin personality under gexc: static Mach-O programs that make XNU's
// system calls and Mach traps directly (fixtures/darwin, over its own small
// C library), each checked against the same source run on the Mac itself
// (lib/darwin.mjs), under gexc and on the page's runtime. What a new
// process is handed on its stack; files and directories in Darwin's layouts
// and error numbers; fork, wait4, kill and execve; mmap on Darwin's page
// and Mach's virtual memory traps; the clocks and the calls that wait;
// signals, delivered through XNU's frame to a handler behind its
// trampoline; threads, made as libpthread asks for them (on the Mac, by its
// libpthread), waiting on ulock, and on what libpthread's mutexes,
// conditions and read-write locks ask of the kernel (psynch);
// posix_spawn, with its file actions and attributes; what a process asks
// about itself and its
// machine (sysctl, csops, proc_info, statfs); the terminal's ioctls, with
// BSD's termios; sockets, with BSD's sockaddr, msghdr and cmsghdr; and
// Mach ports within a task: rights, messages, semaphores, and the kernel's
// own ports answering MIG requests. And the calls an old program makes:
// stat, statfs and getdirentries as they were before 64-bit inodes.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('hello', await d.build('hello'), ['one', 'two words'], { page: true });
  // `files` says what the filesystem under it says (an empty directory's
  // size, a name's longest length): the Mac's own under gexc, and so not
  // the page's in-memory one.
  await d.compare('files', await d.build('files'));
  // (And getattrlist: a file's numbers are the filesystem's.)
  await d.compare('attr', await d.build('attr'));
  // (Likewise the calls from before 64-bit inodes: it lists a directory.)
  await d.compare('old', await d.build('old'));
  for (const name of ['proc', 'mem', 'time', 'sig', 'thr', 'tty', 'mach', 'psynch', 'kqueue', 'vnode', 'calls', 'shm', 'fifo', 'jobs']) await d.compare(name, await d.build(name), [], { page: true });
  // Processes that a fork parted making code at one address, in turns:
  // under gexc only, as the page's runtime refuses a fixed mapping in one
  // process (ENOMEM) where another, alive, has mapped since their fork.
  await d.compare('forkcode', await d.build('forkcode'));
  // posix_spawn. A file without permission to be run is refused under
  // gexc; the page's files have no modes, and it is run (and fails as the
  // child).
  const spawn = await d.build('spawn');
  await d.compare('spawn', spawn, [], { page: true });
  await d.compare('spawn, a file that may not be run', spawn, ['modes']);
  // The commpage, where gex puts it and where XNU does.
  await d.compare('commpage', await d.build('commpage'), [], { page: true });
  const sys = await d.build('sys');
  await d.compare('sys', sys, ['one', 'two'], { page: true });
  // Under gexc the root is the Mac's own, and the system a program is told
  // it is on is the one that root's SystemVersion.plist names: the Mac's.
  await d.compare('sys, which system', sys, ['version']);
  // A closed port on the loopback refuses under gexc; on the page the
  // loopback is the proxy's to answer for, so that question is asked only
  // here.
  const sock = await d.build('sock');
  await d.compare('sock', sock, [], { page: true });
  await d.compare('sock, a closed port', sock, ['refusal']);
  d.n.clean();
}
