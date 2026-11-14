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
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>

__attribute__((constructor)) static void bastion_noclip_init(void) { unsetenv("LD_PRELOAD"); }

/* The caller frees the result with SDL_free, so allocate it with SDL_malloc when SDL is
 * loaded (it always is in gamescope); calloc is what SDL_malloc defaults to otherwise. */
static char *empty_string(void) {
    void *(*sdl_malloc)(size_t) = (void *(*)(size_t))dlsym(RTLD_DEFAULT, "SDL_malloc");
    char *s = sdl_malloc ? sdl_malloc(1) : calloc(1, 1);
    if (s) *s = '\0';
    return s;
}

int SDL_SetClipboardText(const char *text) { (void)text; return 0; }
char *SDL_GetClipboardText(void) { return empty_string(); }
int SDL_HasClipboardText(void) { return 0; }
int SDL_SetPrimarySelectionText(const char *text) { (void)text; return 0; }
char *SDL_GetPrimarySelectionText(void) { return empty_string(); }
int SDL_HasPrimarySelectionText(void) { return 0; }
