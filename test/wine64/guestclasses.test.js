// cases.mjs guestNative with the DLL's guest entries as class adapters
// (the ICALL pass's GEXW_ENTRY=classes): convmatrix, lateload and cbthrow
// against a gexcc-win DLL (GEX_TEST_GEXCC_WIN) whose functions guest code
// reaches through one adapter per class; under gexc.
import { guestNative } from './cases.mjs';
import { wine64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 1800 };

export default async function (t) {
  const w = wine64(t, { door: 0 });
  await guestNative(t, w, {}, 'classes: ', null, { classes: true });
  w.clean();
}
