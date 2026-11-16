// wine64/guestclasses on the page: the gexcc-win DLL's guest entries as
// class adapters, each thread a Worker with its own table (native PE core,
// GEX_TEST_WINE64_NATIVE).
import { guestNative } from '../wine64/cases.mjs';
import { wine64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 2400 };

export default async function (t) {
  const w = wine64(t, { door: 1, native: true });
  await guestNative(t, w, { page: {}, timeout: 900000 }, 'page classes: ', null, { classes: true });
  w.clean();
}
