/* Owned, fixed-size UCRT malloc/calloc/realloc/free qualification workload. */
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

static void *blocks[128];

__declspec(dllexport) const char * __cdecl hold(int argc, const char **argv) {
    (void)argc; (void)argv;
    if (blocks[0]) return "-1";
    for (size_t i = 0; i < 128; ++i) {
        blocks[i] = i % 2 ? calloc(1024, 4) : malloc(4096);
        if (!blocks[i]) return "-2";
        memset(blocks[i], 0x5a, 4096);
    }
    void *larger = realloc(blocks[0], 8192);
    if (!larger) return "-3";
    blocks[0] = larger;
    /* UCRT must fail this request without freeing or changing the old block. */
    void *failed = realloc(blocks[1], SIZE_MAX);
    if (failed) { blocks[1] = failed; return "-4"; }
    if (((unsigned char *)blocks[1])[0] != 0x5a) return "-5";
    return "128";
}

__declspec(dllexport) const char * __cdecl release(int argc, const char **argv) {
    (void)argc; (void)argv;
    /* Exercise UCRT's zero-size realloc semantics as well as free. */
    if (blocks[0]) { void *zero = realloc(blocks[0], 0); if (zero) return "-6"; blocks[0] = NULL; }
    for (size_t i = 1; i < 128; ++i) { free(blocks[i]); blocks[i] = NULL; }
    return "0";
}
