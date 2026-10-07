#!/usr/bin/env python3
"""Draws the app icon and writes assets/app.ico plus assets/app.res.

app.res is the compiled Windows resource (icon + version info) that build.rs
links into the exe, so building needs no resource compiler. Re-run this after
changing the design, the app name or the version:

    python3 scripts/make-icon.py
"""
import re, struct, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NAME = re.search(r'APP_NAME: &str = "([^"]+)"', (ROOT / "src/config.rs").read_text()).group(1)
VERSION = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)

TILE = (20, 22, 28)
CELL = (228, 231, 238)
ACCENT = (87, 158, 255)


def shapes():
    """(x, y, w, h, radius, colour) in unit coordinates: a dark tile holding a
    ragged grid of cells with one lit."""
    yield (0.04, 0.04, 0.92, 0.92, 0.21, TILE)
    w, h, gx, gy, x0, y0 = 0.18, 0.15, 0.045, 0.075, 0.19, 0.20
    for r, cols in enumerate((3, 2, 3)):
        for c in range(cols):
            lit = (r, c) == (1, 1)
            yield (x0 + c * (w + gx), y0 + r * (h + gy), w, h, 0.04, ACCENT if lit else CELL)


def render(n):
    """n x n premultiplied RGBA, anti-aliased by exact edge distance."""
    px = [[0.0, 0.0, 0.0, 0.0] for _ in range(n * n)]
    for x, y, w, h, radius, colour in shapes():
        cx, cy, hw, hh, rad = (x + w / 2) * n, (y + h / 2) * n, w / 2 * n, h / 2 * n, radius * n
        rad = min(rad, hw, hh)
        for py in range(max(0, int(cy - hh - 1)), min(n, int(cy + hh + 2))):
            for qx in range(max(0, int(cx - hw - 1)), min(n, int(cx + hw + 2))):
                dx = max(abs(qx + 0.5 - cx) - (hw - rad), 0.0)
                dy = max(abs(py + 0.5 - cy) - (hh - rad), 0.0)
                cover = min(max(0.5 - ((dx * dx + dy * dy) ** 0.5 - rad), 0.0), 1.0)
                if cover:
                    p = px[py * n + qx]
                    for i in range(3):
                        p[i] = colour[i] * cover + p[i] * (1 - cover)
                    p[3] = 255 * cover + p[3] * (1 - cover)
    return px


def png(n, px):
    raw = b"".join(
        b"\0" + bytes(
            v for p in px[y * n:(y + 1) * n]
            for v in ([round(c * 255 / p[3]) for c in p[:3]] + [round(p[3])] if p[3] else [0, 0, 0, 0])
        )
        for y in range(n)
    )
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def dib(n, px):
    """Icon bitmap: header, bottom-up BGRA (straight alpha), then an empty AND mask."""
    header = struct.pack("<IiiHHIIiiII", 40, n, n * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = b"".join(
        bytes(
            v for p in px[y * n:(y + 1) * n]
            for v in ([round(p[2] * 255 / p[3]), round(p[1] * 255 / p[3]), round(p[0] * 255 / p[3]), round(p[3])]
                      if p[3] else [0, 0, 0, 0])
        )
        for y in reversed(range(n))
    )
    return header + rows + bytes(((n + 31) // 32) * 4 * n)


def resource(kind, ident, data, flags):
    header = struct.pack("<IIHHHHIHHII", len(data), 32, 0xFFFF, kind, 0xFFFF, ident, 0, flags, 0x0409, 0, 0)
    return header + data + bytes(-len(data) % 4)


def version_node(key, value=b"", value_len=0, text=True, children=()):
    body = (key + "\0").encode("utf-16-le")
    body += bytes(-(6 + len(body)) % 4) + value
    for child in children:
        body += bytes(-(6 + len(body)) % 4) + child
    return struct.pack("<HHH", 6 + len(body), value_len, 1 if text else 0) + body


def version_info():
    parts = [int(p) for p in VERSION.split(".")] + [0]
    ms, ls = parts[0] << 16 | parts[1], parts[2] << 16 | parts[3]
    fixed = struct.pack("<IIIIIIIIIIIII", 0xFEEF04BD, 0x10000, ms, ls, ms, ls, 0x3F, 0, 0x40004, 1, 0, 0, 0)
    def string(key, value):
        return version_node(key, (value + "\0").encode("utf-16-le"), len(value) + 1)
    strings = [
        string("FileDescription", NAME),
        string("ProductName", NAME),
        string("FileVersion", VERSION),
        string("ProductVersion", VERSION),
        string("OriginalFilename", NAME + ".exe"),
        string("InternalName", NAME),
    ]
    return version_node("VS_VERSION_INFO", fixed, len(fixed), text=False, children=[
        version_node("StringFileInfo", children=[version_node("040904B0", children=strings)]),
        version_node("VarFileInfo", children=[
            version_node("Translation", struct.pack("<HH", 0x0409, 0x04B0), 4, text=False)]),
    ])


def main():
    images = []
    for n in (16, 20, 24, 32, 40, 48, 64, 256):
        px = render(n)
        images.append((n, png(n, px) if n == 256 else dib(n, px)))

    entry = lambda n, data, tail: struct.pack("<BBBBHHI", n % 256, n % 256, 0, 0, 1, 32, len(data)) + tail
    ico = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for n, data in images:
        ico += entry(n, data, struct.pack("<I", offset))
        offset += len(data)
    (ROOT / "assets/app.ico").write_bytes(ico + b"".join(data for _, data in images))

    res = struct.pack("<IIHHHHIHHII", 0, 32, 0xFFFF, 0, 0xFFFF, 0, 0, 0, 0, 0, 0)
    group = struct.pack("<HHH", 0, 1, len(images))
    for i, (n, data) in enumerate(images, start=1):
        res += resource(3, i, data, 0x1010)                     # RT_ICON
        group += entry(n, data, struct.pack("<H", i))
    res += resource(14, 1, group, 0x1030)                       # RT_GROUP_ICON, id 1
    res += resource(16, 1, version_info(), 0x0030)              # RT_VERSION
    (ROOT / "assets/app.res").write_bytes(res)
    (ROOT / "assets/icon-256.png").write_bytes(images[-1][1])
    print(f"{NAME} {VERSION}: assets/app.ico, assets/app.res")


if __name__ == "__main__":
    main()
