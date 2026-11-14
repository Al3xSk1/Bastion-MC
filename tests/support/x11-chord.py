#!/usr/bin/python3 -I
"""Press a key chord (held modifiers, then a key) once on an X display with XTEST.

For tests/browser.rs (BASTION_BROWSER_LOGIN_DISPLAY): Ctrl+Q closes the `browser login`
window the way a person would, so that Firefox ends normally and writes its profile.

The keys go to whatever window has the keyboard focus on $DISPLAY. Use it only on a private
display that nothing else is using (tests/support/private-x11.sh) - never on a desktop session.

usage: x11-chord.py <keysym in hex>...   e.g. x11-chord.py ffe3 71   (Control_L, q)
The last keysym is tapped while the ones before it are held. Prints "pressed ..." when done.
"""
import ctypes, os, sys, time

def main():
    keysyms = [int(k, 16) for k in sys.argv[1:]]
    if not keysyms:
        sys.exit("usage: x11-chord.py <keysym>...")
    x11 = ctypes.CDLL("libX11.so.6")
    xtst = ctypes.CDLL("libXtst.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xtst.XTestQueryExtension.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(ctypes.c_int)] * 4
    xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

    name = os.environ.get("DISPLAY", "")
    display = x11.XOpenDisplay(name.encode())
    if not display:
        sys.exit(f"x11-chord: cannot open display {name!r}")
    ints = [ctypes.c_int() for _ in range(4)]
    if not xtst.XTestQueryExtension(display, *[ctypes.byref(i) for i in ints]):
        sys.exit("x11-chord: the display has no XTEST extension")
    codes = [x11.XKeysymToKeycode(display, k) for k in keysyms]
    if 0 in codes:
        sys.exit(f"x11-chord: no key produces one of {[hex(k) for k in keysyms]}")
    down = []
    try:
        for code in codes:
            xtst.XTestFakeKeyEvent(display, code, 1, 0)
            x11.XFlush(display)
            down.append(code)
            time.sleep(0.05)
        time.sleep(0.1)
    finally:
        # Never leave a key down, whatever happens.
        for code in reversed(down):
            xtst.XTestFakeKeyEvent(display, code, 0, 0)
            x11.XFlush(display)
            time.sleep(0.02)
    x11.XCloseDisplay(display)
    print("pressed " + " ".join(hex(k) for k in keysyms), flush=True)

main()
