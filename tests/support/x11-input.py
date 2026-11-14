#!/usr/bin/python3 -I
"""Keys and mouse clicks on an X display with the XTEST extension, when told to; the geometry
of the display's first client window. For tests/support/phone-shots.sh (package mod-ui).

Like tests/support/x11-key.py, the input goes to whatever has the focus on $DISPLAY: use it
only on a private display that nothing else is using (tests/support/private-x11.sh).

Reads lines from standard input and answers each with one line:
  key <keysym hex> <ms>     press the key that produces the keysym, hold it, release it
  click <x> <y> [button]    move the pointer to root coordinates (x, y), press and release a
                            mouse button (1 left, 2 middle, 3 right; default 1)
  geometry                  "<x> <y> <w> <h>" of the first window in _NET_CLIENT_LIST, in root
                            coordinates (or "none")
Prints "ready" once connected. Ends when standard input ends.
"""
import ctypes, os, sys, time


def main():
    x11 = ctypes.CDLL("libX11.so.6")
    xtst = ctypes.CDLL("libXtst.so.6")
    vp, ul, ui, ci = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_int
    x11.XOpenDisplay.restype = vp
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XKeysymToKeycode.argtypes = [vp, ul]
    x11.XFlush.argtypes = [vp]
    x11.XDefaultRootWindow.restype = ul
    x11.XDefaultRootWindow.argtypes = [vp]
    x11.XInternAtom.restype = ul
    x11.XInternAtom.argtypes = [vp, ctypes.c_char_p, ci]
    x11.XGetWindowProperty.argtypes = [vp, ul, ul, ctypes.c_long, ctypes.c_long, ci, ul,
                                       ctypes.POINTER(ul), ctypes.POINTER(ci), ctypes.POINTER(ul),
                                       ctypes.POINTER(ul), ctypes.POINTER(ctypes.c_void_p)]
    x11.XFree.argtypes = [vp]
    x11.XGetGeometry.argtypes = [vp, ul, ctypes.POINTER(ul), ctypes.POINTER(ci), ctypes.POINTER(ci),
                                 ctypes.POINTER(ui), ctypes.POINTER(ui), ctypes.POINTER(ui), ctypes.POINTER(ui)]
    x11.XTranslateCoordinates.argtypes = [vp, ul, ul, ci, ci, ctypes.POINTER(ci), ctypes.POINTER(ci),
                                          ctypes.POINTER(ul)]
    xtst.XTestQueryExtension.argtypes = [vp] + [ctypes.POINTER(ci)] * 4
    xtst.XTestFakeKeyEvent.argtypes = [vp, ui, ci, ul]
    xtst.XTestFakeMotionEvent.argtypes = [vp, ci, ci, ci, ul]
    xtst.XTestFakeButtonEvent.argtypes = [vp, ui, ci, ul]

    name = os.environ.get("DISPLAY", "")
    display = x11.XOpenDisplay(name.encode())
    if not display:
        sys.exit(f"x11-input: cannot open display {name!r}")
    ints = [ci() for _ in range(4)]
    if not xtst.XTestQueryExtension(display, *[ctypes.byref(i) for i in ints]):
        sys.exit("x11-input: the display has no XTEST extension")
    root = x11.XDefaultRootWindow(display)

    def first_client():
        atom = x11.XInternAtom(display, b"_NET_CLIENT_LIST", 0)
        kind, fmt, count, after, data = ul(), ci(), ul(), ul(), ctypes.c_void_p()
        if x11.XGetWindowProperty(display, root, atom, 0, 64, 0, 33, ctypes.byref(kind), ctypes.byref(fmt),
                                  ctypes.byref(count), ctypes.byref(after), ctypes.byref(data)) != 0:
            return None
        try:
            if count.value == 0 or not data.value:
                return None
            return ctypes.cast(data, ctypes.POINTER(ul))[0]
        finally:
            if data.value:
                x11.XFree(data)

    print("ready", flush=True)
    for line in sys.stdin:
        fields = line.split()
        if not fields:
            continue
        if fields[0] == "key" and len(fields) == 3:
            keysym, millis = int(fields[1], 16), min(int(fields[2]), 10_000)
            keycode = x11.XKeysymToKeycode(display, keysym)
            if keycode == 0:
                print(f"no key produces keysym {keysym:#x}", flush=True)
                continue
            xtst.XTestFakeKeyEvent(display, keycode, 1, 0)
            x11.XFlush(display)
            try:
                time.sleep(millis / 1000)
            finally:
                xtst.XTestFakeKeyEvent(display, keycode, 0, 0)
                x11.XFlush(display)
            print(f"pressed keysym={keysym:#x} keycode={keycode} held={millis}ms", flush=True)
        elif fields[0] == "click" and len(fields) in (3, 4):
            x, y = int(fields[1]), int(fields[2])
            button = int(fields[3]) if len(fields) == 4 else 1
            xtst.XTestFakeMotionEvent(display, -1, x, y, 0)
            x11.XFlush(display)
            time.sleep(0.08)
            xtst.XTestFakeButtonEvent(display, button, 1, 0)
            x11.XFlush(display)
            time.sleep(0.08)
            xtst.XTestFakeButtonEvent(display, button, 0, 0)
            x11.XFlush(display)
            print(f"clicked {x},{y} button {button}", flush=True)
        elif fields[0] == "geometry":
            window = first_client()
            if not window:
                print("none", flush=True)
                continue
            parent, gx, gy = ul(), ci(), ci()
            w, h, border, depth = ui(), ui(), ui(), ui()
            x11.XGetGeometry(display, window, ctypes.byref(parent), ctypes.byref(gx), ctypes.byref(gy),
                             ctypes.byref(w), ctypes.byref(h), ctypes.byref(border), ctypes.byref(depth))
            rx, ry, child = ci(), ci(), ul()
            x11.XTranslateCoordinates(display, window, root, 0, 0, ctypes.byref(rx), ctypes.byref(ry),
                                      ctypes.byref(child))
            print(f"{rx.value} {ry.value} {w.value} {h.value}", flush=True)
        else:
            print(f"not understood: {line.strip()!r}", flush=True)


main()
