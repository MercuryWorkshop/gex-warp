/* An image is relocated where it is mapped, before anything runs in it, as
 * Windows maps one (test/wow64/imgreloc): its own .exe mapped twice more
 * as an image section (MapViewOfFile of a SEC_IMAGE mapping, which no loader
 * relocates) has its header's ImageBase and a pointer in its data at the
 * view; and a copy of itself started suspended, as another process sees it
 * before it runs (its PEB's ImageBaseAddress, then its header and data
 * there), is relocated already. Prints "imgreloc map ok" for each view or what differs,
 * "imgreloc child ok" or what differs, then "imgreloc done"; the child
 * prints nothing.
 * gex-nt-arch: x64
 * gex-nt-variant: imgreloc -Wl,--dynamicbase,--high-entropy-va
 */
#include <windows.h>
#include <winternl.h>
#include <stdio.h>

extern IMAGE_DOS_HEADER __ImageBase;

/* an absolute pointer in data: a relocation the image's own code never ran */
static void *self_ptr = &self_ptr;

typedef NTSTATUS (WINAPI *query_fn)( HANDLE, PROCESSINFOCLASS, void *, ULONG, ULONG * );

static ULONGLONG header_base( const char *base )
{
    const IMAGE_NT_HEADERS64 *nt = (const void *)(base + ((const IMAGE_DOS_HEADER *)base)->e_lfanew);
    return nt->OptionalHeader.ImageBase;
}

/* the image at `view`, read in this process: whether it is relocated there */
static void check_map( const char *view )
{
    ULONG_PTR rva = (char *)&self_ptr - (char *)&__ImageBase;
    ULONGLONG hb = header_base( view );
    void *ptr = *(void **)(view + rva);

    if (view == (char *)&__ImageBase) { printf( "imgreloc map at the running image's base\n" ); return; }
    if (hb == (ULONG_PTR)view && ptr == view + rva) printf( "imgreloc map ok\n" );
    else printf( "imgreloc map at %p: header base %I64x, pointer %p (want %p)\n", view, hb, ptr, view + rva );
}

static void check_child( const char *exe )
{
    query_fn query = (query_fn)GetProcAddress( GetModuleHandleA( "ntdll.dll" ), "NtQueryInformationProcess" );
    STARTUPINFOA si = { sizeof(si) };
    PROCESS_INFORMATION pi;
    PROCESS_BASIC_INFORMATION pbi;
    char cmd[MAX_PATH + 16], *base = NULL, head[4096];
    ULONG_PTR rva = (char *)&self_ptr - (char *)&__ImageBase;
    void *ptr = NULL;
    SIZE_T got;
    ULONGLONG hb;
    DWORD code = 0;

    snprintf( cmd, sizeof(cmd), "\"%s\" child", exe );
    if (!CreateProcessA( exe, cmd, NULL, NULL, FALSE, CREATE_SUSPENDED, NULL, NULL, &si, &pi ))
    { printf( "imgreloc child CreateProcess failed %lu\n", GetLastError() ); return; }
    if (query( pi.hProcess, ProcessBasicInformation, &pbi, sizeof(pbi), NULL ) ||
        !ReadProcessMemory( pi.hProcess, (char *)pbi.PebBaseAddress + 0x10, &base, sizeof(base), &got ) ||
        !ReadProcessMemory( pi.hProcess, base, head, sizeof(head), &got ) ||
        !ReadProcessMemory( pi.hProcess, base + rva, &ptr, sizeof(ptr), &got ))
        printf( "imgreloc child unreadable %lu\n", GetLastError() );
    else
    {
        hb = header_base( head );
        if (hb == (ULONG_PTR)base && ptr == base + rva) printf( "imgreloc child ok\n" );
        else printf( "imgreloc child at %p: header base %I64x, pointer %p (want %p)\n", base, hb, ptr, base + rva );
    }
    ResumeThread( pi.hThread );
    WaitForSingleObject( pi.hProcess, 60000 );
    GetExitCodeProcess( pi.hProcess, &code );
    if (code) printf( "imgreloc child exit %lx\n", code );
    CloseHandle( pi.hThread );
    CloseHandle( pi.hProcess );
}

int main( int argc, char **argv )
{
    char exe[MAX_PATH];
    HANDLE file, mapping;
    char *view, *view2;

    if (argc > 1 && !strcmp( argv[1], "child" )) return self_ptr == &self_ptr ? 0 : 3;

    GetModuleFileNameA( NULL, exe, sizeof(exe) );
    file = CreateFileA( exe, GENERIC_READ | GENERIC_EXECUTE, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL );
    mapping = file == INVALID_HANDLE_VALUE ? NULL :
              CreateFileMappingA( file, NULL, PAGE_EXECUTE_READ | SEC_IMAGE, 0, 0, NULL );
    view = mapping ? MapViewOfFile( mapping, FILE_MAP_READ | FILE_MAP_EXECUTE, 0, 0, 0 ) : NULL;
    view2 = mapping ? MapViewOfFile( mapping, FILE_MAP_READ | FILE_MAP_EXECUTE, 0, 0, 0 ) : NULL;
    if (!view || !view2) printf( "imgreloc map failed %lu\n", GetLastError() );
    else
    {
        /* each view relocated for its own address, none through another's
         * pages, nor through the running program's */
        check_map( view );
        check_map( view2 );
        if (self_ptr != &self_ptr) printf( "imgreloc running image changed: %p\n", self_ptr );
    }

    check_child( exe );
    printf( "imgreloc done\n" );
    return 0;
}
