// An epoll set added to while another thread waits on it wakes that thread
// for the new descriptor (fixtures/native/epoll-add): a parked epoll_wait
// took the set's descriptors as they were when it parked, and one that
// another thread added (async-io's reactor, under glycin's image loaders in
// every GTK program) never woke it. Natively.
import { native } from '../lib/native.mjs';

export const meta = { hosts: ['node'], timeout: 300 };

export default async function (t) {
  const n = native(t);
  const d = n.dir('epoll-add');
  await n.cc(['-O1', '-static', '-pthread', '-o', `${d}/ea`, n.src('epoll-add/ea.c')]);
  const r = await n.gexc(['run', `${d}/ea`], { timeout: 60000 });
  t.check('the waiter is woken by the descriptor added while it waited', /^woken by the pipe$/m.test(r.stdout), `${r.stdout.trim()} / ${r.stderr.trim().split('\n').slice(-3).join(' / ')}`);
}
