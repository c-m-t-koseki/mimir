#!/usr/bin/env python3
"""Convert an SKK dictionary into Mozc TSV for mimir's user_dicts/.

    scripts/skk2mozc.py SKK-JISYO.L > skk-l.tsv
    scripts/skk2mozc.py ~/Library/Application\\ Support/AquaSKK/skk-jisyo.utf8 > skk-user.tsv
    scripts/skk2mozc.py --abbrev SKK-JISYO.L > skk-abbrev.tsv

`--abbrev` takes the entries read in ASCII instead (`github /GitHub/`),
the ones SKK's abbrev input (`/`) converts. They share no reading with
kana input, so even a big dictionary's worth ranks above nothing there.

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
ABBREV = re.compile(r"^[A-Za-z0-9][!-~]*$")


def read_text(path):
    data = open(path, "rb").read()
    for encoding in ("utf-8", "euc_jis_2004", "euc_jp"):
        try:
            return data.decode(encoding)
        except UnicodeDecodeError:
            continue
    return data.decode("euc_jp", errors="replace")


def convert(text, comment, reading_pattern=READING):
    for line in text.splitlines():
        if not line or line.startswith(";"):
            continue
        reading, _, rest = line.partition(" /")
        if not reading_pattern.match(reading):
            continue
        for candidate in rest.split("/"):
            surface = candidate.split(";", 1)[0]
            if surface and not surface.startswith("("):
                yield f"{reading}\t{surface}\t名詞\t{comment}"


def main():
    args = sys.argv[1:]
    abbrev = "--abbrev" in args
    args = [a for a in args if a != "--abbrev"]
    if len(args) != 1:
        sys.exit(__doc__)
    for row in convert(read_text(args[0]), "SKK", ABBREV if abbrev else READING):
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
    rows = list(convert("github /GitHub/\nかいぎ /会議/\n", "SKK", ABBREV))
    assert rows == ["github\tGitHub\t名詞\tSKK"], rows


if __name__ == "__main__":
    _check()
    main()
