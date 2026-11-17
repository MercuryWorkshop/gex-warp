/* Native code calling a native DLL's exports through GetProcAddress's
 * pointers, which are guest addresses: the call goes out through the guest
 * to the export (built with gexcc-win against convdll.c). Every shape
 * conv.h has must agree with the same call through conv_table()'s pointers
 * (table indices, called directly): a struct return beside a call of the
 * same wasm type (`struct s12 f(int)`, `void f(int *, int)`) included.
 * "exitshape: shapes agree", then "exitshape ok". */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "conv.h"

static int bad;
#define SAME(what, a, b) do { if ((a) != (b)) { printf("exitshape: %s differs\n", what); bad++; } } while (0)

static int cb(int x) { return x * 5 + 1; }

static const struct conv_table *t;
static HMODULE m;

int main(int argc, char **argv)
{
    const struct conv_table *(*get)(void);
    struct conv_table e;
    char buf[16];

    m = LoadLibraryA("convdll.dll");
    if (!m || !(get = (void *)GetProcAddress(m, "conv_table"))) { printf("exitshape: no convdll\n"); return 1; }
    t = get();
#define GET(r, name, a) if (!(e.name = (void *)GetProcAddress(m, #name))) { printf("exitshape: no %s\n", #name); return 1; }
    CONV_FUNCS(GET)

    SAME("c_i0", e.c_i0(), t->c_i0());
    SAME("c_i4", e.c_i4(1, -2, 3, 40), t->c_i4(1, -2, 3, 40));
    SAME("c_i12", e.c_i12(1, 2ll << 40, 3, -4, 5, 6, 7, 8, 9, 10, 11, 12), t->c_i12(1, 2ll << 40, 3, -4, 5, 6, 7, 8, 9, 10, 11, 12));
    SAME("c_mix", e.c_mix(1, 2.5, 3, 4.25f, 5ll << 33, 6.5), t->c_mix(1, 2.5, 3, 4.25f, 5ll << 33, 6.5));
    SAME("c_fp8", e.c_fp8(1, 2, 3, 4, 5, 6, 7, 8.5f), t->c_fp8(1, 2, 3, 4, 5, 6, 7, 8.5f));
    SAME("c_f", e.c_f(3.5f), t->c_f(3.5f));
    SAME("c_d", e.c_d(7, 2), t->c_d(7, 2));
    SAME("c_u8", e.c_u8(200), t->c_u8(200));
    SAME("c_s16", e.c_s16(-300), t->c_s16(-300));
    SAME("c_ret_u8", e.c_ret_u8(255), t->c_ret_u8(255));
    SAME("c_ret_s16", e.c_ret_s16(-32768), t->c_ret_s16(-32768));
    SAME("c_i64", e.c_i64(1ll << 50, 3), t->c_i64(1ll << 50, 3));
    SAME("c_ptr", e.c_ptr(buf, 5), t->c_ptr(buf, 5));
    {
        struct s1 v1 = { 3 }; struct s2 v2 = { -4 }; struct s3 v3 = { { 1, 2, 3 } }; struct s4 v4 = { 5 };
        struct s8 v8 = { 6, 7 }; struct s12 v12 = { { 8, 9, 10 } }; struct s16 v16 = { 11, 12ll << 40 };
        struct s24 v24 = { { 13, 14, 15ll << 35 } };
        SAME("c_b1", e.c_b1(1, v1, 2), t->c_b1(1, v1, 2));
        SAME("c_b2", e.c_b2(1, v2, 2), t->c_b2(1, v2, 2));
        SAME("c_b3", e.c_b3(1, v3, 2), t->c_b3(1, v3, 2));
        SAME("c_b4", e.c_b4(1, v4, 2), t->c_b4(1, v4, 2));
        SAME("c_b8", e.c_b8(1, v8, 2), t->c_b8(1, v8, 2));
        SAME("c_b12", e.c_b12(1, v12, 2), t->c_b12(1, v12, 2));
        SAME("c_b16", e.c_b16(1, v16, 2), t->c_b16(1, v16, 2));
        SAME("c_b24", e.c_b24(1, v24, 2), t->c_b24(1, v24, 2));
    }
    SAME("c_r1", e.c_r1(9).a, t->c_r1(9).a);
    SAME("c_r2", e.c_r2(9).a, t->c_r2(9).a);
    SAME("c_r4", e.c_r4(9).a, t->c_r4(9).a);
    SAME("c_r8", e.c_r8(9).b, t->c_r8(9).b);
    SAME("c_r12", e.c_r12(9).a[2], t->c_r12(9).a[2]);
    {
        struct s24 a = e.c_r24(3, 4), b = t->c_r24(3, 4);
        SAME("c_r24", memcmp(&a, &b, sizeof a), 0);
    }
    {
        int x = 0, y = 0;
        e.c_void(&x, 17);
        t->c_void(&y, 17);
        SAME("c_void", x, y);
    }
    SAME("c_call", e.c_call(cb, 6), t->c_call(cb, 6));
    SAME("c_i16", e.c_i16(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16), t->c_i16(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16));
    printf("exitshape: shapes %s\n", bad ? "differ" : "agree");
    printf(bad ? "exitshape: FAIL\n" : "exitshape ok\n");
    return bad != 0;
}
