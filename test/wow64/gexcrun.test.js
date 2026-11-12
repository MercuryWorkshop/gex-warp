// `gexc run X.exe`: a PE program starts Wine itself (crates/gexc/src/nt.rs),
// with the dist from --wine-dist and the prefix from --wine-prefix. A 64-bit
// program, and a 32-bit one (the same Wine, WoW64). The programs are
// ntrun.c's, built for each.
import { wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const w = wow64(t);
  const dist = process.env.GEX_TEST_WOW64_DIST;
    const exe32 = await w.mingw('ntrun');
  const exe64 = await w.mingw('ntrun', { out: 'ntrun64.exe', x64: true });
  for (const [name, exe, bits] of [['pe32', exe32, 32], ['pe64', exe64, 64]]) {
    const pfx = w.pfx(`${name}-pfx`);
    const out = await w.gexcRun(['run', '--wine-dist', dist, '--wine-prefix', pfx, exe], { log: name });
    t.check(`${name}: ntrun ok ${bits}-bit`, new RegExp(`^ntrun ok ${bits}-bit`, 'm').test(out), out.slice(-300));
  }
  w.clean();
}
