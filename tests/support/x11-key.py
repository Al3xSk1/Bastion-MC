#!/usr/bin/python3 -I
"""Press a key on an X display with the XTEST extension, when told to.

For tests/companion_game.rs (BASTION_COMPANION_KEYPRESS): the companion mod's zoom key, really
pressed, so that the game's own input handling is part of what is tested.

The key goes to whatever window has the keyboard focus on $DISPLAY. Use it only on a
private display that nothing else is using (tests/support/private-x11.sh) - never on a
desktop session, and not on a display shared with other tests.

Reads lines from standard input: "<keysym in hex> <milliseconds>" presses the key that
produces the keysym, holds it, and releases it. Ends when standard input ends. Prints one
line per press ("pressed keysym=0x63 keycode=54 held=1500ms") and "ready" once connected.
"""
import ctypes, os, sys, time

def main():
    x11 = ctypes.CDLL("libX11.so.6")
    xtst = ctypes.CDLL("libXtst.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    xtst.XTestQueryExtension.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(ctypes.c_int)] * 4
    xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

    name = os.environ.get("DISPLAY", "")
    display = x11.XOpenDisplay(name.encode())
    if not display:
        sys.exit(f"x11-key: cannot open display {name!r}")
    ints = [ctypes.c_int() for _ in range(4)]
    if not xtst.XTestQueryExtension(display, *[ctypes.byref(i) for i in ints]):
        sys.exit("x11-key: the display has no XTEST extension")
    print("ready", flush=True)
    for line in sys.stdin:
        fields = line.split()
        if len(fields) != 2:
            continue
        keysym, millis = int(fields[0], 16), min(int(fields[1]), 10_000)
        keycode = x11.XKeysymToKeycode(display, keysym)
        if keycode == 0:
            print(f"no key produces keysym {keysym:#x}", flush=True)
            continue
        xtst.XTestFakeKeyEvent(display, keycode, 1, 0)
        x11.XFlush(display)
        try:
            time.sleep(millis / 1000)
        finally:
            # Never leave the key down, whatever happens.
            xtst.XTestFakeKeyEvent(display, keycode, 0, 0)
            x11.XFlush(display)
        print(f"pressed keysym={keysym:#x} keycode={keycode} held={millis}ms", flush=True)

main()
