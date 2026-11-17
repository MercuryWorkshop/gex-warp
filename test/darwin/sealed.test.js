// A private mapping of a file on a read-only mount (fixtures/darwin/
// sealed.c), against the Mac: gex maps the whole host pages of such a file
// from the file, copy-on-write (both views of the guest's memory one
// object), as XNU maps a library's segments, rather than copying them into
// every process. The program runs twice under gexc, its file on a mount
// bound read-only (mapped: GEX_MEM_TRACE says so) and on one bound writable
// (copied), and both must print what the Mac prints: the file's bytes, stores the mapping's
// alone, a fork's child's its own, memory mapped over it zero.
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 120 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('sealed');
  const files = join(d.dir, 'sealed-files');
  mkdirSync(files, { recursive: true });
  const size = 6 * 16384 + 100;
  const data = Buffer.alloc(size);
  for (let i = 0; i < size; i++) data[i] = (i * 7 + (i >> 12)) & 0xff;
  writeFileSync(join(files, 'data'), data);
  const want = await d.mac(prog, [join(files, 'data')]);
  t.check('the Mac: nothing failed', want.code === 0 && !/^FAIL /m.test(want.stdout), want.stdout.split('\n').filter((l) => l.startsWith('FAIL ')).join(' / ') || `exit ${want.code}`);
  for (const [how, bind] of [['read-only', '--bind-ro'], ['writable', '--bind']]) {
    const got = await d.n.gexc(['run', bind, `${files}=/sealed`, '--env', 'T_VAR=yes', prog.gex, '/sealed/data'], { cwd: files, timeout: 120000, env: { GEX_MEM_TRACE: '1' } });
    const same = got.stdout === want.stdout && got.code === want.code;
    t.check(`its file on a ${how} mount: as on the Mac`, same, same ? '' : `exit ${got.code}: ${got.stdout.split('\n').filter((l) => l.startsWith('FAIL ')).join(' / ')} ${got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
    // (GEX_MEM_TRACE: which it mapped from the file)
    const mapped = /mapped from the file/.test(got.stderr);
    t.check(`its file on a ${how} mount: ${bind === '--bind-ro' ? '' : 'not '}mapped from the file`, mapped === (bind === '--bind-ro'), got.stderr.split('\n').filter((l) => l.startsWith('gex-mem')).join(' / '));
  }
  d.n.clean();
}
