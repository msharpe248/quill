#!/usr/bin/python3
"""Lays out the window of Quill's disk image, without the Finder.

A disk image that opens as a picture with the app on one side and
Applications on the other is a folder with a `.DS_Store` in it: the file
the Finder keeps a folder's window, view and icon positions in. The usual
way to get one is to script the Finder into arranging the window and let
it write the file, which needs the Finder, a screen, and permission to
drive it. This writes the file itself.

    layout.py VOLUME          write VOLUME/.DS_Store
    layout.py --show FILE     print what a .DS_Store written here says

VOLUME is the mounted, writable image, already holding `Quill.app`,
`Applications` and `.background/background.tiff`. It must be the volume
the image is made from, not a folder copied into one afterwards: the
picture is named in the file by an alias, which goes by where the picture
is on that volume.

The numbers are the artwork's: `background.png` is 768 by 512 points, with
room left for an icon at each of the two places below.

Only the standard library, and nothing newer than Python 3.9, which is
what `/usr/bin/python3` is.
"""

import os
import plistlib
import struct
import sys

# The window's content, in points: the size of the picture.
WIDTH, HEIGHT = 768, 512
# The title bar above it, which the window's bounds include. If the
# Finder's is taller than this the picture loses that much of its foot,
# which is empty.
TITLE_BAR = 28
# Where the window opens on the screen: its top left corner.
LEFT, TOP = 200, 120
ICON_SIZE = 96
TEXT_SIZE = 12
# Where each icon's centre goes, from the top left of the content.
ICONS = {"Quill.app": (192, 282), "Applications": (576, 282)}
PICTURE = (".background", "background.tiff")
# What the volume is called, and where a volume of that name is mounted.
VOLUME_NAME = "Quill"

# Seconds from the Mac's epoch (1904) to Unix's (1970).
MAC_EPOCH = 2082844800


def mac_date(seconds):
    return int(seconds) + MAC_EPOCH


def pascal(text, size):
    """A length byte and the text, in `size` bytes."""
    raw = text.encode("mac_roman")[: size - 1]
    return bytes([len(raw)]) + raw.ljust(size - 1, b"\0")


def alias(volume, folder, name):
    """An alias record (version 2) for `volume/folder/name`.

    It says where the file is three ways, and whoever resolves it tries
    them in turn: by the numbers the volume knows the folder and the file
    by, by its path in the old style, and by its path.
    """
    root = os.stat(volume)
    parent = os.stat(os.path.join(volume, folder))
    target = os.stat(os.path.join(volume, folder, name))
    volume_made = getattr(root, "st_birthtime", root.st_ctime)
    made = getattr(target, "st_birthtime", target.st_ctime)

    fixed = struct.pack(
        ">h28sI2shI64sII4s4shhI2s10s",
        0,  # a file, not a folder
        pascal(VOLUME_NAME, 28),
        mac_date(volume_made),
        b"H+",
        0,  # a fixed disk
        parent.st_ino,
        pascal(name, 64),
        target.st_ino,
        mac_date(made),
        b"\0\0\0\0",
        b"\0\0\0\0",
        -1,
        -1,
        0,
        b"\0\0",
        b"\0" * 10,
    )

    def tagged(tag, data):
        return struct.pack(">hH", tag, len(data)) + data + (b"\0" if len(data) % 2 else b"")

    def unicode_name(text):
        return struct.pack(">H", len(text)) + text.encode("utf-16-be")

    extras = b"".join(
        [
            tagged(0, folder.encode("mac_roman")),
            tagged(16, struct.pack(">q", int(mac_date(volume_made) * 65536))),
            tagged(17, struct.pack(">q", int(mac_date(made) * 65536))),
            tagged(1, struct.pack(">I", parent.st_ino)),
            tagged(2, ":".join([VOLUME_NAME, folder, name]).encode("mac_roman")),
            tagged(14, unicode_name(name)),
            tagged(15, unicode_name(VOLUME_NAME)),
            tagged(18, ("/" + folder + "/" + name).encode("utf-8")),
            tagged(19, ("/Volumes/" + VOLUME_NAME).encode("utf-8")),
            struct.pack(">hH", -1, 0),
        ]
    )
    size = 8 + len(fixed) + len(extras)
    return struct.pack(">4shh", b"\0\0\0\0", size, 2) + fixed + extras


def records(volume):
    """What the file says, as (name, code, kind, value), in its order."""
    window = {
        "ContainerShowSidebar": False,
        "PreviewPaneVisibility": False,
        "ShowPathbar": False,
        "ShowSidebar": False,
        "ShowStatusBar": False,
        "ShowTabView": False,
        "ShowToolbar": False,
        "SidebarWidth": 0,
        "WindowBounds": "{{%d, %d}, {%d, %d}}" % (LEFT, TOP, WIDTH, HEIGHT + TITLE_BAR),
    }
    icons = {
        "arrangeBy": "none",
        "backgroundColorBlue": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorRed": 1.0,
        "backgroundImageAlias": alias(volume, *PICTURE),
        "backgroundType": 2,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "gridSpacing": 100.0,
        "iconSize": float(ICON_SIZE),
        "labelOnBottom": True,
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
        "showIconPreview": False,
        "showItemInfo": False,
        "textSize": float(TEXT_SIZE),
        "viewOptionsVersion": 1,
    }
    binary = lambda value: plistlib.dumps(value, fmt=plistlib.FMT_BINARY)
    found = [
        (".", b"bwsp", b"blob", binary(window)),
        (".", b"icvp", b"blob", binary(icons)),
        # The folder opens as icons.
        (".", b"vSrn", b"long", 1),
    ]
    for name, (x, y) in ICONS.items():
        if not os.path.lexists(os.path.join(volume, name)):
            raise SystemExit("layout.py: there is no %s in %s" % (name, volume))
        place = struct.pack(">IIII", x, y, 0xFFFFFFFF, 0xFFFF0000)
        found.append((name, b"Iloc", b"blob", place))
    # The file is a tree in the order the Finder looks names up in: by
    # name with no regard to case, then by code.
    return sorted(found, key=lambda record: (record[0].lower(), record[1]))


def encoded(record):
    name, code, kind, value = record
    out = struct.pack(">I", len(name)) + name.encode("utf-16-be") + code + kind
    if kind == b"blob":
        return out + struct.pack(">I", len(value)) + value
    if kind == b"long":
        return out + struct.pack(">I", value)
    raise ValueError(kind)


# The file is a small heap ("Bud1") of blocks, each a power of two long,
# and a table saying where each is. Everything in it is counted from just
# after the file's first four bytes. Three blocks are enough: the table,
# the tree's header, and one leaf with every record in it.
HEADER = 32
TREE_AT, TREE_BITS = 32, 5
TABLE_AT, TABLE_BITS = 2048, 11
LEAF_AT, LEAF_BITS = 4096, 12
PAGE = 1 << LEAF_BITS


def store(found):
    leaf = struct.pack(">II", 0, len(found)) + b"".join(encoded(record) for record in found)
    if len(leaf) > PAGE:
        raise SystemExit("layout.py: the layout does not fit one page of the file")
    tree = struct.pack(">IIIII", 2, 0, len(found), 1, PAGE)

    addresses = [TABLE_AT | TABLE_BITS, TREE_AT | TREE_BITS, LEAF_AT | LEAF_BITS]
    table = struct.pack(">II", len(addresses), 0)
    table += b"".join(struct.pack(">I", address) for address in addresses)
    table += b"\0" * (4 * (256 - len(addresses)))
    table += struct.pack(">I", 1) + bytes([4]) + b"DSDB" + struct.pack(">I", 1)
    # What is free, by size: every power of two that is not one of the
    # three blocks above or the header before them.
    used = {TREE_BITS, TABLE_BITS, LEAF_BITS}
    for bits in range(32):
        free = [1 << bits] if 5 < bits < 31 and bits not in used else []
        table += struct.pack(">I", len(free)) + b"".join(struct.pack(">I", at) for at in free)
    if len(table) > 1 << TABLE_BITS:
        raise SystemExit("layout.py: the table of blocks does not fit")

    body = bytearray(LEAF_AT + PAGE)
    body[0:HEADER] = struct.pack(">4sIII16s", b"Bud1", TABLE_AT, len(table), TABLE_AT, b"\0" * 16)
    body[TREE_AT : TREE_AT + len(tree)] = tree
    body[TABLE_AT : TABLE_AT + len(table)] = table
    body[LEAF_AT : LEAF_AT + len(leaf)] = leaf
    return struct.pack(">I", 1) + bytes(body)


def read(data):
    """The records of a file written by `store`, found the way a reader
    finds them: through the header, the table and the tree."""
    if struct.unpack(">I", data[:4])[0] != 1 or data[4:8] != b"Bud1":
        raise SystemExit("layout.py: not a .DS_Store")
    body = data[4:]
    table_at, table_size, again = struct.unpack(">III", body[4:16])
    if table_at != again:
        raise SystemExit("layout.py: the table of blocks is named in two places")
    table = body[table_at : table_at + table_size]
    count = struct.unpack(">I", table[:4])[0]
    addresses = struct.unpack(">%dI" % count, table[8 : 8 + 4 * count])
    at = 8 + 4 * ((count + 255) // 256 * 256)
    names = {}
    for _ in range(struct.unpack(">I", table[at : at + 4])[0]):
        length = table[at + 4]
        name = table[at + 5 : at + 5 + length]
        names[name] = struct.unpack(">I", table[at + 5 + length : at + 9 + length])[0]
        at += 5 + length

    def block(number):
        address = addresses[number]
        start, size = address & ~0x1F, 1 << (address & 0x1F)
        return body[start : start + size]

    root, _, total, _, _ = struct.unpack(">IIIII", block(names[b"DSDB"])[:20])
    leaf = block(root)
    below, count = struct.unpack(">II", leaf[:8])
    if below != 0 or count != total:
        raise SystemExit("layout.py: more than one page of records")
    at, found = 8, []
    for _ in range(count):
        length = struct.unpack(">I", leaf[at : at + 4])[0]
        name = leaf[at + 4 : at + 4 + 2 * length].decode("utf-16-be")
        at += 4 + 2 * length
        code, kind = leaf[at : at + 4], leaf[at + 4 : at + 8]
        at += 8
        if kind == b"blob":
            size = struct.unpack(">I", leaf[at : at + 4])[0]
            value = leaf[at + 4 : at + 4 + size]
            at += 4 + size
        else:
            value = struct.unpack(">I", leaf[at : at + 4])[0]
            at += 4
        found.append((name, code, kind, value))
    return found


def show(found):
    for name, code, kind, value in found:
        code = code.decode("ascii")
        if code == "Iloc":
            print("%s: icon centred at %d, %d" % ((name,) + struct.unpack(">II", value[:8])))
        elif code in ("bwsp", "icvp"):
            for key, said in sorted(plistlib.loads(value).items()):
                if isinstance(said, bytes):
                    said = "%d bytes" % len(said)
                print("%s: %s %s = %s" % (name, code, key, said))
        else:
            print("%s: %s = %s" % (name, code, value))


def main(arguments):
    if len(arguments) == 2 and arguments[0] == "--show":
        with open(arguments[1], "rb") as file:
            show(read(file.read()))
        return
    if len(arguments) != 1 or arguments[0].startswith("-"):
        raise SystemExit(__doc__.strip())
    volume = arguments[0]
    if not os.path.isfile(os.path.join(volume, *PICTURE)):
        raise SystemExit("layout.py: there is no %s in %s" % ("/".join(PICTURE), volume))
    found = records(volume)
    data = store(found)
    # What was written is read back before it is kept.
    if read(data) != found:
        raise SystemExit("layout.py: what was written does not read back as it was")
    with open(os.path.join(volume, ".DS_Store"), "wb") as file:
        file.write(data)


if __name__ == "__main__":
    main(sys.argv[1:])
