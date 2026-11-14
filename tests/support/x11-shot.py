#!/usr/bin/python3 -I
"""Save the largest window an X display shows as a PNG, with plain XGetImage (no shared
memory, which a private display in a namespace of its own does not share with the caller;
and not the root window, which a rootless Xwayland cannot read back).

For tests/browser.rs (BASTION_BROWSER_LOGIN_DISPLAY): a picture of the `browser login`
window. Use it only on a private display (tests/support/private-x11.sh).

usage: x11-shot.py <out.png>   (the display is $DISPLAY). Prints "WxH, N distinct colours".
"""
import ctypes, os, struct, sys, zlib

class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int), ("height", ctypes.c_int),
        ("border_width", ctypes.c_int), ("depth", ctypes.c_int), ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong), ("class_", ctypes.c_int), ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int), ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong), ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int), ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p),
    ]

class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int),
        ("red_mask", ctypes.c_ulong), ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
    ]

def png(path, width, height, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + row for row in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 6)))
        f.write(chunk(b"IEND", b""))

def main():
    if len(sys.argv) != 2:
        sys.exit("usage: x11-shot.py <out.png>")
    x11 = ctypes.CDLL("libX11.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XDisplayWidth.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x11.XDisplayHeight.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x11.XGetImage.restype = ctypes.POINTER(XImage)
    x11.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint,
                              ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
    name = os.environ.get("DISPLAY", "")
    display = x11.XOpenDisplay(name.encode())
    if not display:
        sys.exit(f"x11-shot: cannot open display {name!r}")
    root = x11.XDefaultRootWindow(display)
    x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                               ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
                               ctypes.POINTER(ctypes.c_uint)]
    x11.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(XWindowAttributes)]
    root_ret, parent_ret = ctypes.c_ulong(), ctypes.c_ulong()
    children, count = ctypes.POINTER(ctypes.c_ulong)(), ctypes.c_uint()
    x11.XQueryTree(display, root, ctypes.byref(root_ret), ctypes.byref(parent_ret), ctypes.byref(children),
                   ctypes.byref(count))
    best, size = None, (0, 0)
    for i in range(count.value):
        attrs = XWindowAttributes()
        if x11.XGetWindowAttributes(display, children[i], ctypes.byref(attrs)) and attrs.map_state == 2:
            if attrs.width * attrs.height > size[0] * size[1]:
                best, size = children[i], (attrs.width, attrs.height)
    if best is None:
        sys.exit("x11-shot: no window is shown")
    width, height = size
    image = x11.XGetImage(display, best, 0, 0, width, height, 0xFFFFFFFF, 2)  # AllPlanes, ZPixmap
    if not image:
        sys.exit("x11-shot: XGetImage failed")
    img = image.contents
    if img.bits_per_pixel != 32:
        sys.exit(f"x11-shot: {img.bits_per_pixel} bits per pixel is not handled")
    data = ctypes.string_at(img.data, img.bytes_per_line * img.height)
    rows, colours = [], set()
    for y in range(img.height):
        line = data[y * img.bytes_per_line: y * img.bytes_per_line + img.width * 4]
        rgb = bytearray(img.width * 3)
        rgb[0::3], rgb[1::3], rgb[2::3] = line[2::4], line[1::4], line[0::4]  # BGRX, little-endian
        rows.append(bytes(rgb))
        if y % 16 == 0:
            colours.update(bytes(rgb[i:i + 3]) for i in range(0, len(rgb), 48))
    png(sys.argv[1], img.width, img.height, rows)
    print(f"{img.width}x{img.height}, {len(colours)} distinct colours", flush=True)

main()
