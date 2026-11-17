// A large native module on the page never passes through the shim's heap:
// a gexcc-win program (class adapters, so its slot types are read too)
// whose module is padded past 160 MiB with a custom section, run on the
// page under the native PE core. Its process's runtime heap and largest
// single claim (GEX_DEBUG_EXIT's exit lines) stay far below the module's
// size: the shim reads its sections but the code, data and names, its
// names and content key from the file in pieces, and JS reads the bytes
// it compiles from the file.
import { randomFillSync } from 'node:crypto';
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { Skip } from '../lib/t.js';
import { wine64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 2400 };

const PAD_MIB = 160;

/** A custom section `name` of `size` bytes of noise, as a module's tail. */
function customSection(name, size) {
  const leb = (n) => {
    const out = [];
    do {
      let b = n & 0x7f;
      n = Math.floor(n / 128);
      if (n) b |= 0x80;
      out.push(b);
    } while (n);
    return Buffer.from(out);
  };
  const nm = Buffer.from(name);
  const body = Buffer.alloc(size - nm.length - 1);
  randomFillSync(body);
  return Buffer.concat([Buffer.from([0]), leb(size), leb(nm.length), nm, body]);
}

export default async function (t) {
  const w = wine64(t, { door: 1, native: true });
  const src = join(w.work, 'modbytes.c');
  writeFileSync(src, '#include <stdio.h>\nstatic int add(int a, int b) { return a + b; }\nint (*volatile f)(int, int) = add;\n' +
    'int main(void) { printf("modbytes %d\\n", f(40, 2)); return 0; }\n');
  let exe;
  try {
    exe = await w.gexccWin(src, 'modbytes.exe', ['-O1'], { env: { GEXW_ENTRY: 'classes' } });
  } catch (e) {
    if (e instanceof Skip) throw e;
    t.check('modbytes: build', false, String(e.message));
    return;
  }
  appendFileSync(`${exe}.wasm`, customSection('gex.test.pad', PAD_MIB << 20));
  const r = await w.nt({ want: /modbytes 42/, page: {}, name: 'modbytes', exe, extra: [`${exe}.wasm`], env: { GEX_DEBUG_EXIT: '1' } });
  if (r) {
    const lines = r.err.split('\n').filter((l) => /arena heap \d+ MiB, largest claim \d+ MiB \{[^}]*modbytes/i.test(l));
    const said = lines.map((l) => ({ heap: Number(/arena heap (\d+)/.exec(l)[1]), largest: Number(/largest claim (\d+)/.exec(l)[1]) }));
    t.check('modbytes: its process said its arena at exit', said.length > 0, w.detail(r));
    for (const s of said) t.say(`modbytes: runtime heap ${s.heap} MiB, largest claim ${s.largest} MiB, module ${PAD_MIB} MiB and more`);
    for (const s of said) {
      t.check(`modbytes: runtime heap ${s.heap} MiB, under a quarter of the ${PAD_MIB} MiB module`, s.heap < PAD_MIB / 4, lines.join(' / '));
      t.check(`modbytes: largest claim ${s.largest} MiB, under a sixteenth of the module`, s.largest < PAD_MIB / 16, lines.join(' / '));
    }
  }
  w.clean();
}
