#!/usr/bin/env python3
"""Convert an SKK dictionary into Mozc TSV for mimir's user_dicts/.

    scripts/skk2mozc.py SKK-JISYO.L > skk-l.tsv
    scripts/skk2mozc.py ~/Library/Application\\ Support/AquaSKK/skk-jisyo.utf8 > skk-user.tsv

Only okuri-nasi entries carry over: an okuri-ari key (`かk /書/`) names a
stem plus the romaji of its inflection, which has no reading-to-surface
form in Mozc TSV. Annotations (`;...`), Lisp candidates (`(concat ...)`)
and prefix/suffix keys (`>てき`, `ご>`) are dropped. The file's encoding
(UTF-8 or EUC-JP) is detected. Large outputs belong in a binary dict:

    cargo run --release --bin mimir-dict -- build --format mozc skk-l.tsv -o skk-l.bin
"""

import re
import sys

READING = re.compile(r"^[ぁ-ゖー]+$")


def read_text(path):
    data = open(path, "rb").read()
    for encoding in ("utf-8", "euc_jis_2004", "euc_jp"):
        try:
            return data.decode(encoding)
        except UnicodeDecodeError:
            continue
    return data.decode("euc_jp", errors="replace")


def convert(text, comment):
    for line in text.splitlines():
        if not line or line.startswith(";"):
            continue
        reading, _, rest = line.partition(" /")
        if not READING.match(reading):
            continue
        for candidate in rest.split("/"):
            surface = candidate.split(";", 1)[0]
            if surface and not surface.startswith("("):
                yield f"{reading}\t{surface}\t名詞\t{comment}"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    for row in convert(read_text(sys.argv[1]), "SKK"):
        print(row)


def _check():
    rows = list(convert(
        ";; okuri-ari entries.\n"
        "かk /書/描/\n"
        ";; okuri-nasi entries.\n"
        "かいぎ /会議;meeting/懐疑/\n"
        ">てき /的/\n"
        "きょう /(concat \"今日\")/今日/\n",
        "SKK",
    ))
    assert rows == [
        "かいぎ\t会議\t名詞\tSKK",
        "かいぎ\t懐疑\t名詞\tSKK",
        "きょう\t今日\t名詞\tSKK",
    ], rows


if __name__ == "__main__":
    _check()
    main()
