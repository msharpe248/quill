"""Makes the small files the demo mailbox attaches to its mail.

Everything here is invented: drawn shapes, a made-up roadmap, a one-second
tone. Run it from this folder to regenerate them; the JPEG needs macOS's
`sips`. The results are committed, so nothing needs this at build time.
"""
import math
import struct
import subprocess
import wave
import zlib


def png(path, width, height, pixel):
    """Writes an RGB PNG whose pixel at (x, y) is `pixel(x, y)`."""
    rows = b"".join(
        b"\x00" + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height)
    )

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n")
        out.write(chunk(b"IHDR", header))
        out.write(chunk(b"IDAT", zlib.compress(rows, 9)))
        out.write(chunk(b"IEND", b""))


def harbour(x, y):
    """A sunset over water: a sky that warms towards the horizon, a sun, and
    a darker sea with a few bright ripples."""
    w, h, horizon = 320, 200, 120
    if y < horizon:
        t = y / horizon
        r, g, b = int(40 + 200 * t), int(60 + 110 * t), int(120 + 40 * (1 - t))
        if (x - 220) ** 2 + (y - 88) ** 2 < 26 ** 2:
            r, g, b = 255, 236, 170
    else:
        t = (y - horizon) / (h - horizon)
        r, g, b = int(30 + 60 * (1 - t)), int(50 + 60 * (1 - t)), int(90 + 60 * (1 - t))
        if (y % 9 == 0) and abs(x - 220) < 40 + 80 * t and (x // 7) % 2 == 0:
            r, g, b = 250, 200, 140
    return r, g, b


def ridge(x, y):
    """Three ranges of hills, each paler than the one in front, under a sky."""
    sky = (150 + y // 4, 190 + y // 6, 235)
    for depth, (base, swing, period, shade) in enumerate(
        [(150, 26, 61.0, 60), (125, 34, 97.0, 105), (95, 22, 43.0, 150)]
    ):
        crest = base - swing * (0.5 + 0.5 * math.sin(x / period + depth * 1.7))
        if y >= crest:
            return shade - 20, shade + 10, shade
    return tuple(min(255, c) for c in sky)


def chart(x, y):
    """A bar chart on white: four bars, a baseline."""
    w, h = 320, 200
    if y in (170, 171) and 30 <= x <= 290:
        return 60, 60, 70
    for index, height in enumerate([60, 95, 80, 130]):
        left = 50 + index * 62
        if left <= x < left + 40 and 170 - height <= y < 170:
            return [(86, 120, 200), (86, 160, 140), (220, 150, 80), (190, 90, 110)][index]
    return 252, 252, 250


def pdf(path, title, lines):
    """Writes a one-page PDF: a title and some lines of text in Helvetica."""
    text = ["BT", "/F1 20 Tf", "72 720 Td", f"({title}) Tj", "/F1 12 Tf"]
    for line in lines:
        text += ["0 -22 Td", f"({line}) Tj"]
    text.append("ET")
    stream = "\n".join(text).encode("ascii")
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
        b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n" % (len(objects) + 1)
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref,
    )
    with open(path, "wb") as f:
        f.write(out)


def tone(path):
    """One second of a soft two-note chime, 8 kHz, 8-bit mono."""
    rate = 8000
    with wave.open(path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(1)
        out.setframerate(rate)
        frames = bytearray()
        for n in range(rate):
            t = n / rate
            note = 660 if t < 0.5 else 880
            fade = min(1.0, (0.5 - abs((t % 0.5) - 0.25)) * 8)
            frames.append(int(128 + 70 * fade * math.sin(2 * math.pi * note * t)))
        out.writeframes(bytes(frames))


png("harbour.png", 320, 200, harbour)
png("ridge.png", 320, 200, ridge)
png("chart.png", 320, 200, chart)
subprocess.run(
    ["sips", "-s", "format", "jpeg", "-s", "formatOptions", "70", "ridge.png", "--out", "ridge.jpg"],
    check=True,
    capture_output=True,
)
subprocess.run(["rm", "ridge.png"], check=True)
pdf(
    "roadmap.pdf",
    "Q4 roadmap",
    [
        "October: ship the sync rewrite.",
        "November: search.",
        "December: whatever the first two months break.",
        "",
        "Open questions",
        "1. Is the index format settled?",
        "2. Two weeks for the offline queue: realistic?",
        "3. Who is on call over the holidays?",
    ],
)
pdf(
    "invoice-2026-114.pdf",
    "Invoice 2026-114",
    [
        "From: Ortega Design, 12 Example Street",
        "To: Example Corp",
        "",
        "Brand refresh, phase two          4,200.00",
        "Icon set, 24 icons                  960.00",
        "",
        "Total due within 30 days          5,160.00",
    ],
)
pdf(
    "contract-v3.pdf",
    "Services agreement, draft 3",
    [
        "Between Example Corp and Haddad Consulting.",
        "",
        "4.2 Term: twelve months from the start date.",
        "7.1 Either party may end this agreement on 60 days' notice.",
        "9.3 Liability is limited to the fees paid in the prior year.",
    ],
)
with open("offsite-agenda.txt", "w") as f:
    f.write(
        "Offsite agenda\n"
        "==============\n\n"
        "09:30  Coffee, and what went well last quarter\n"
        "10:15  The roadmap, in three slides\n"
        "12:00  Lunch by the water\n"
        "13:30  Breakouts: search, sync, and the thing nobody owns\n"
        "16:00  Walk\n"
    )
tone("kitchen-timer.wav")
