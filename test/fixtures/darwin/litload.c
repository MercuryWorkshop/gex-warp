// dlopen the bundle litload-bundle.c is built as (argv[1]) and read its
// globals back through its literal loads, before and after they are
// written: what each is in the file, then what was stored, twice over
// (test/darwin/litload.test.js). Ruby's fiddle.bundle stores its module in
// Init_fiddle and Init_fiddle_function reads it back so.
#include <dlfcn.h>
#include <stdio.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    void *h = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!h) {
        printf("dlopen: %s\n", dlerror());
        return 1;
    }
    void (*set)(long, long) = (void (*)(long, long))dlsym(h, "lit_set");
    long (*common)(void) = (long (*)(void))dlsym(h, "lit_load_common");
    long (*data)(void) = (long (*)(void))dlsym(h, "lit_load_data");
    long (*swap)(long) = (long (*)(long))dlsym(h, "lit_swap");
    if (!set || !common || !data || !swap) {
        printf("dlsym: %s\n", dlerror());
        return 1;
    }
    printf("as loaded: common %#lx, data %#lx\n", common(), data());
    set(0x1234, 0x5678);
    printf("set once: common %#lx, data %#lx\n", common(), data());
    set(0xabcd, 0);
    printf("set again: common %#lx, data %#lx\n", common(), data());
    long first = swap(0x1111);
    long second = swap(0x2222);
    printf("swap: %#lx, then %#lx\n", first, second);
    printf("after swap: common %#lx\n", common());
    return 0;
}
