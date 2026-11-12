// What darwin/high and darwin/high-native check of fixtures/darwin/high.c's
// lines against a Mac's: each of the Mac's lines, said alike, under the
// name before its colon. Two the Mac says are known to differ: pyrus's
// dyld does not tell the kernel where its list of images is (TASK_DYLD_INFO
// answers none), and a page has no page protection, so a store into
// PROT_NONE does not fault there and there is no fault to say anything of.
export function compare(t, where, mac, got, code) {
  const key = (l) => l.slice(0, l.indexOf(': ') < 0 ? l.length : l.indexOf(': '));
  // (a name said twice is two lines: the first of them, then the second)
  const named = (list) => {
    const seen = new Map();
    return list.map((l) => {
      const k = key(l), n = (seen.get(k) ?? 0) + 1;
      seen.set(k, n);
      return [n > 1 ? `${k} (${n})` : k, l];
    });
  };
  const lines = got.split('\n').filter(Boolean);
  const found = new Map(named(lines));
  for (const [k, want] of named(mac.split('\n').filter(Boolean))) {
    const l = found.get(k) ?? '(nothing)';
    if (k === "dyld's image list") {
      t.known(`${where}: ${k}`, l === want, l, "pyrus's dyld does not tell the kernel where its image list is (TASK_DYLD_INFO answers none)");
    } else if (k.startsWith('a fault') && where === 'the page') {
      t.known(`${where}: ${k}`, l === want, l, 'a page has no page protection: a store into PROT_NONE does not fault there');
    } else {
      t.eq(`${where}: ${k}`, l, want);
    }
  }
  t.eq(`${where}: it says nothing is below 4 GiB, and exits 0`, [lines.at(-1), code], ['nothing below 4 GiB', 0]);
}
