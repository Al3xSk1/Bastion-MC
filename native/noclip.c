/*
 * Preloaded into the nested compositor (gamescope) only: cut the clipboard bridge.
 *
 * gamescope copies the CLIPBOARD and PRIMARY selections between the session's X server and
 * its inner display, in both directions, through four SDL calls. It has no option to turn
 * that off. Interposing those four calls makes both directions see nothing:
 *
 *   - session -> game: gamescope asks SDL for the session's selection and gets "".
 *   - game -> session: gamescope hands the game's selection to SDL, which drops it, so
 *     nothing in the session ever becomes the selection owner on the game's behalf.
 *
 * Nothing else changes. The constructor removes LD_PRELOAD from the environment again, so
 * that the programs gamescope starts (Xwayland, xkbcomp, the launcher's ready child) are
 * not given this library.
 *
 * A preload fails silently: the dynamic linker skips a library it may not load (a program
 * with file capabilities or set-id bits) or cannot map, and a compositor that reaches SDL
 * some other way never calls these definitions. So the library says what it found, from
 * inside the compositor: the constructor leaves a status word in the environment variable
 * BASTION_MC_NOCLIP, the launcher's ready child (started by the compositor, so it inherits
 * that environment) passes it on, and the launcher claims "clipboard cut" only for "active".
 * No word at all means the library never ran. See src/sandbox/nested.rs for what "active"
 * does and does not establish.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>

/* The caller frees the result with SDL_free, so allocate it with SDL_malloc when SDL is
 * loaded (it always is in gamescope); calloc is what SDL_malloc defaults to otherwise. */
static char *empty_string(void) {
    void *(*sdl_malloc)(size_t) = (void *(*)(size_t))dlsym(RTLD_DEFAULT, "SDL_malloc");
    char *s = sdl_malloc ? sdl_malloc(1) : calloc(1, 1);
    if (s) *s = '\0';
    return s;
}

/* The three behaviours. They are static, and the SDL names below are aliases for them, so
 * that this file can tell its own definitions (by address) from someone else's of the same
 * name: a reference to an exported name from inside a shared library is itself subject to
 * the search order that is being checked. */
static int drop_text(const char *text) { (void)text; return 0; }
static char *no_text(void) { return empty_string(); }
static int has_no_text(void) { return 0; }

#define IS(impl) __attribute__((alias(#impl), visibility("default")))
int SDL_SetClipboardText(const char *text) IS(drop_text);
char *SDL_GetClipboardText(void) IS(no_text);
int SDL_HasClipboardText(void) IS(has_no_text);
int SDL_SetPrimarySelectionText(const char *text) IS(drop_text);
char *SDL_GetPrimarySelectionText(void) IS(no_text);
int SDL_HasPrimarySelectionText(void) IS(has_no_text);

/* What the definitions above are worth in this process. */
static const char *status(void) {
    static const struct { const char *name; void *own; } cut[] = {
        /* The four calls that carry the bridge first. */
        {"SDL_SetClipboardText", (void *)drop_text},
        {"SDL_GetClipboardText", (void *)no_text},
        {"SDL_SetPrimarySelectionText", (void *)drop_text},
        {"SDL_GetPrimarySelectionText", (void *)no_text},
        {"SDL_HasClipboardText", (void *)has_no_text},
        {"SDL_HasPrimarySelectionText", (void *)has_no_text},
    };
    /* SDL 3's interface for clipboard data of any type. Nothing here interposes it. */
    static const char *const uncovered[] = {
        "SDL_SetClipboardData", "SDL_GetClipboardData", "SDL_HasClipboardData",
        "SDL_ClearClipboardData", "SDL_GetClipboardMimeTypes",
    };
    unsigned i;
    /* 1. A call by name from the program lands here: the search order every dynamic
     *    reference goes through finds this library's definition first. It does not if the
     *    program defines the function itself (SDL linked in and exported). */
    for (i = 0; i < sizeof cut / sizeof cut[0]; i++)
        if (dlsym(RTLD_DEFAULT, cut[i].name) != cut[i].own) return "shadowed";
    /* 2. Behind this library stands an SDL that provides the bridge functions, i.e. the
     *    program gets them from the dynamic linker. If none does, it has SDL linked in
     *    privately or loads it later by handle, and never asks for these names. */
    for (i = 0; i < 4; i++)
        if (!dlsym(RTLD_NEXT, cut[i].name)) return "no-sdl";
    /* 3. The program cannot be using a clipboard interface that is not cut. */
    for (i = 0; i < sizeof uncovered / sizeof uncovered[0]; i++)
        if (dlsym(RTLD_DEFAULT, uncovered[i])) return "data-interface";
    return "active";
}

__attribute__((constructor)) static void bastion_noclip_init(void) {
    unsetenv("LD_PRELOAD");
    setenv("BASTION_MC_NOCLIP", status(), 1);
}
