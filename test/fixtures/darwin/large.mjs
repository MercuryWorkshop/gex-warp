// A C program of large functions, for test/darwin/large.test.js: one of
// `blocks` basic blocks over 64 values live at once (at -O0 a function of
// more than a megabyte, past an arm64 conditional branch's reach), frames
// of 70 KB, 1.1 MB and 3 MB (the last on the main thread and on a thread's
// stack) and one of 2.5 MB from alloca, each probed page by page; switches
// of `cases` dense cases (a jump table), 1500 sparse ones and 1200 with
// fall-through in a loop; a call of `args` arguments of mixed types with
// two structures, most on the stack, and a variadic one of 30; and a
// computed goto among 600 labels. Each prints a hash of what it computed,
// so a translation that differs anywhere shows on its line.
export function largeProgram({ blocks = 4000, cases = 3000, args = 48, seed = 1234 } = {}) {
  // A seeded generator (xorshift), so a program is the same every time.
  let s = BigInt(seed) | 1n;
  const M = (1n << 64n) - 1n;
  const next = () => {
    s ^= (s << 13n) & M;
    s ^= s >> 7n;
    s ^= (s << 17n) & M;
    return s;
  };
  const rnd = (n) => Number(next() % BigInt(n));
  const big = (bits) => next() & ((1n << BigInt(bits)) - 1n);
  const pick = (xs) => xs[rnd(xs.length)];
  const o = [];
  const P = (x) => o.push(x);
  P('#include <stdio.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <pthread.h>\n#include <stdarg.h>\n');
  P('static volatile uint64_t vin = 7;\n#define NOI __attribute__((noinline))\n');
  P('static uint64_t mix(uint64_t h, uint64_t v) { h ^= v + 0x9e3779b97f4a7c15ULL + (h << 6) + (h >> 2); return h; }\n');
  const V = 64;
  P(`NOI uint64_t blocks(uint64_t x) {\n  uint64_t ${Array.from({ length: V }, (_, i) => `v${i} = x + ${i}`).join(', ')};\n`);
  for (let b = 0; b < blocks; b++) {
    const [a, c, d] = [rnd(V), rnd(V), rnd(V)];
    const k = 1 + rnd((1 << 30) - 1);
    P(`  if ((v${a} ^ ${k}ULL) & ${1 << rnd(6)}) { v${c} = v${c} ${pick(['+', '^', '-', '*', '|'])} (v${d} >> ${1 + rnd(6)}); } else { v${d} += ${k}ULL; }\n`);
    if (b % 500 === 250) P(`  if (v${a} == 0x1234567) { printf("rare %llu\\n", (unsigned long long)v${c}); }\n`);
  }
  P('  uint64_t h = 0;\n' + Array.from({ length: V }, (_, i) => `  h = mix(h, v${i});\n`).join('') + '  return h;\n}\n');
  for (const [name, size] of [['frame70k', 70000], ['frame1m', 1100000], ['frame3m', 3 << 20]]) {
    P(`NOI uint64_t ${name}(uint64_t x) {\n  volatile unsigned char buf[${size}];\n  for (size_t i = 0; i < ${size}; i += 4093) buf[i] = (unsigned char)(x + i);\n` +
      `  buf[${size - 1}] = 9;\n  uint64_t h = 0;\n  for (size_t i = 0; i < ${size}; i += 4093) h = mix(h, buf[i]);\n  return mix(h, buf[${size - 1}]);\n}\n`);
  }
  P('NOI uint64_t callee(uint64_t *p, uint64_t x) { p[0] += x; return p[0] * 3; }\n');
  P('NOI uint64_t frame_mixed(uint64_t x) {\n  uint64_t lo = x * 5;\n  volatile uint64_t arr[40000];\n  uint64_t hi = x * 7;\n  arr[0] = x; arr[39999] = x + 1;\n' +
    '  uint64_t r = callee((uint64_t *)&arr[20000], x);\n  return mix(mix(mix(lo, hi), r), arr[0] + arr[39999] + arr[20000]);\n}\n');
  P('NOI uint64_t dyn(uint64_t n) { volatile unsigned char *p = __builtin_alloca(n); for (uint64_t i = 0; i < n; i += 4000) p[i] = (unsigned char)i; p[n - 1] = 5; ' +
    'uint64_t h = 0; for (uint64_t i = 0; i < n; i += 4000) h = mix(h, p[i]); return mix(h, p[n - 1]); }\n');
  P('NOI uint64_t sw_dense(unsigned k) {\n  switch (k) {\n');
  for (let i = 0; i < cases; i++) P(`  case ${i}: return ${big(40)}ULL ^ k;\n`);
  P('  default: return 77;\n  }\n}\n');
  const keys = [...new Set(Array.from({ length: 1500 }, () => big(48)))].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  P('NOI uint64_t sw_sparse(uint64_t k) {\n  switch (k) {\n');
  keys.forEach((k, i) => P(`  case ${k}ULL: return ${i};\n`));
  P('  default: return 99999;\n  }\n}\n');
  P(`static const uint64_t sparse_keys[] = {${keys.filter((_, i) => i % 37 === 0).map((k) => `${k}ULL`).join(',')}};\n`);
  P('NOI uint64_t sw_loop(uint64_t n) {\n  uint64_t h = 1;\n  for (uint64_t i = 0; i < n; i++) {\n    switch ((i * 2654435761u) % 1200) {\n');
  for (let i = 0; i < 1200; i++) P(`    case ${i}: h = h * ${3 + rnd(997)} + ${i}; ${rnd(5) ? 'break;' : '/* falls */'}\n`);
  P('    }\n  }\n  return h;\n}\n');
  const types = Array.from({ length: args }, () => pick(['uint64_t', 'int32_t', 'double', 'float', 'uint8_t', 'int16_t']));
  const fp = (t) => t === 'double' || t === 'float';
  P('struct S3 { uint64_t a, b, c; };\nstruct S2 { uint32_t a; float f; };\n');
  P(`NOI uint64_t many(${types.map((t, i) => `${t} a${i}`).join(', ')}, struct S3 s3, struct S2 s2, uint64_t last) {\n  uint64_t h = 0;\n` +
    types.map((t, i) => (fp(t) ? `  h = mix(h, (uint64_t)(int64_t)(a${i} * 1000.0));\n` : `  h = mix(h, (uint64_t)a${i});\n`)).join('') +
    '  h = mix(h, s3.a + s3.b * 3 + s3.c * 5); h = mix(h, s2.a + (uint64_t)(s2.f * 8)); return mix(h, last);\n}\n');
  const vals = types.map((t, i) => (fp(t) ? `(${t})(x * 0.5 + ${i}.25)` : `(${t})(x * ${i + 3} + ${rnd(1000)})`));
  P(`NOI uint64_t call_many(uint64_t x) { struct S3 s3 = {x, x + 1, x + 2}; struct S2 s2 = {(uint32_t)x * 9, 2.5f}; return many(${vals.join(', ')}, s3, s2, x ^ 0xabcdef); }\n`);
  P('NOI uint64_t va(int n, ...) { va_list ap; va_start(ap, n); uint64_t h = 0; for (int i = 0; i < n; i++) { if (i % 3 == 2) h = mix(h, (uint64_t)(va_arg(ap, double) * 4)); ' +
    'else h = mix(h, va_arg(ap, uint64_t)); } va_end(ap); return h; }\n');
  P(`NOI uint64_t call_va(uint64_t x) { return va(30${Array.from({ length: 30 }, (_, i) => (i % 3 === 2 ? `, (double)(x + ${i}) / 4` : `, (uint64_t)(x * ${i})`)).join('')}); }\n`);
  P(`NOI uint64_t cgoto(uint64_t n) {\n  static void *tab[] = {${Array.from({ length: 600 }, (_, i) => `&&L${i}`).join(',')}};\n  uint64_t h = 3, i = 0;\n  goto *tab[0];\n`);
  for (let i = 0; i < 600; i++) P(`L${i}: h = h * 31 + ${i}; if (++i >= n) return h; goto *tab[(h >> 7) % 600];\n`);
  P('}\n');
  P('static void *thr(void *a) { uint64_t *out = a; *out = frame3m(*out); return 0; }\n');
  P('int main(int argc, char **argv) {\n  (void)argv;\n  uint64_t x = vin + (uint64_t)argc;\n  setvbuf(stdout, NULL, _IONBF, 0);\n');
  const line = (label, expr) => P(`  printf("${label} %llu\\n", (unsigned long long)(${expr}));\n`);
  line('blocks', 'blocks(x)');
  line('frame70k', 'frame70k(x)');
  line('frame1m', 'frame1m(x)');
  line('frame_mixed', 'frame_mixed(x)');
  P('  { pthread_attr_t at; pthread_attr_init(&at); pthread_attr_setstacksize(&at, 16 << 20); pthread_t t; uint64_t v = x; pthread_create(&t, &at, thr, &v); ' +
    'pthread_join(t, 0); printf("frame3m in a thread %llu\\n", (unsigned long long)v); }\n');
  line('frame3m on the main thread', 'frame3m(x)');
  line('alloca 2.5m', 'dyn(2500000 + x)');
  P(`  { uint64_t h = 0; for (unsigned k = 0; k < ${cases + 10}; k += 7) h = mix(h, sw_dense(k)); printf("sw_dense %llu\\n", (unsigned long long)h); }\n`);
  P('  { uint64_t h = 0; for (size_t i = 0; i < sizeof sparse_keys / 8; i++) h = mix(h, sw_sparse(sparse_keys[i])); h = mix(h, sw_sparse(x)); ' +
    'printf("sw_sparse %llu\\n", (unsigned long long)h); }\n');
  line('sw_loop', 'sw_loop(5000 + x)');
  line('many', 'call_many(x)');
  line('va', 'call_va(x)');
  line('cgoto', 'cgoto(4000 + x)');
  P('  return 0;\n}\n');
  return o.join('');
}
