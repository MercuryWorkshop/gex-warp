// What a new Darwin process is handed: its arguments, its environment and
// the `apple` vector, on the stack as XNU lays them out.
#include "t.h"

int main(int argc, char **argv, char **envp, char **apple) {
    t_printf("hello from darwin\n");
    t_printf("argc %d\n", argc);
    // argv[0] is the path the program was started by, which differs between
    // the two runs.
    for (int i = 1; i < argc; i++) t_printf("argv[%d] %s\n", i, argv[i]);
    t_check("argv ends", argv[argc] == 0);
    t_printf("T_VAR %s\n", t_getenv(envp, "T_VAR"));
    // The values are the kernel's own business; that libSystem finds the
    // keys it reads at start is the contract.
    t_check("apple executable_path", t_getenv(apple, "executable_path") != 0);
    t_check("apple ptr_munge", t_getenv(apple, "ptr_munge") != 0);
    t_check("apple main_stack", t_getenv(apple, "main_stack") != 0);
#ifdef T_STATIC
    // libSystem takes these two out of the vector before main runs, so only
    // a program with no libSystem sees them as the kernel left them.
    const char *guard = t_getenv(apple, "stack_guard");
    if (!guard || !t_getenv(apple, "malloc_entropy")) t_printf("FAIL the guards\n");
    else if (guard[t_strlen(guard) - 1] != '0' || guard[t_strlen(guard) - 2] != '0') t_printf("FAIL the guard's low byte\n");
#endif
    t_dprintf(2, "to stderr\n");
    t_show("write to a closed descriptor", SYS(SYS_write, 77, "x", 1));
    return 42;
}
