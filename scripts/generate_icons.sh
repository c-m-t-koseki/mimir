#!/bin/sh
# Renders icon.svg into the app icons the frontends ship: README's icon.png,
# the macOS app's mimir.icns, and the Windows settings window's icon.png /
# icon.ico (also the input method's icon in the Windows input switcher).
# macOS only: QuickLook draws the SVG, sips resizes, iconutil packs.
#
# Usage: scripts/generate_icons.sh
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

qlmanage -t -s 1024 -o "$tmp" "$root/icon.svg" >/dev/null 2>&1
big="$tmp/icon.svg.png"
if [ ! -f "$big" ]; then
    echo "generate_icons: QuickLook did not render icon.svg" >&2
    exit 1
fi

# $1 pixels square from the 1024 render, into $2.
resize() { sips -z "$1" "$1" "$big" --out "$2" >/dev/null; }

cp "$big" "$root/icon.png"

set="$tmp/mimir.iconset"
mkdir "$set"
for n in 16 32 128 256 512; do
    resize "$n" "$set/icon_${n}x${n}.png"
    resize $((n * 2)) "$set/icon_${n}x${n}@2x.png"
done
iconutil -c icns "$set" -o "$root/mimir-im/macos/resources/mimir.icns"

win="$root/mimir-im/windows/settings/icons"
resize 512 "$win/icon.png"
sizes="16 24 32 48 64 128 256"
for n in $sizes; do resize "$n" "$tmp/win$n.png"; done
# An .ico of PNG images (Windows Vista and later read those).
python3 - "$win/icon.ico" $(for n in $sizes; do echo "$tmp/win$n.png"; done) <<'PY'
import struct, sys
out, *paths = sys.argv[1:]
images = [open(p, "rb").read() for p in paths]
sizes = [struct.unpack(">II", png[16:24])[0] for png in images]
offset = 6 + 16 * len(images)
entries = b""
for size, png in zip(sizes, images):
    # A width / height byte of 0 means 256.
    entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
with open(out, "wb") as f:
    f.write(struct.pack("<HHH", 0, 1, len(images)) + entries + b"".join(images))
PY
