#!/usr/bin/env python3
"""Re-cost a mimir system dictionary with Mozc's, and add verb forms.

    cargo run --release --bin mimir-dict -- view -a dict.bin > dict.tsv
    scripts/mozc_costs.py dict.tsv mozc/src/data/dictionary_oss > dict.json
    cargo run --release --bin mimir-dict -- build dict.json -o dict.bin

See docs/dictionary.md.

The shipped dictionary takes its costs from Sudachi, which ranks for
morphological analysis rather than for typing: 描く read かく costs 99999
there, while Mozc's open-source dictionary (dictionary_oss), made for an
IME, puts it right after 書く. Two changes, both from that dictionary:

- A (reading, surface) pair both dictionaries have takes Mozc's cost,
  mapped linearly onto this dictionary's scale (the two scales differ,
  and agree little: Spearman 0.12 over the 0.56M shared pairs).
- Verb stems Mozc lists for the past and te forms (書い, 待っ, 読ん, 食べ)
  gain those forms as entries (かいた → 書いた / 描いた), which the
  dictionary otherwise lacks: Mozc joins them at run time by part of
  speech, mimir looks up whole readings.

Pairs only Mozc has are otherwise left out. Mozc's dictionary is IPAdic
plus Google's additions and the Okinawa dictionary; see
THIRD_PARTY_LICENSES before distributing the result.
"""

import json
import sys
from collections import defaultdict

# The auxiliary a stem takes for the past and te forms: だ/で after the
# voiced 音便 (読ん, 泳い of a ガ行 verb), た/て otherwise.
VOICED = ("だ", "で")
VOICELESS = ("た", "て")

# Generated forms cost a little more than their stem, as the joining does
# in Mozc (書い + た costs more than 書い alone).
FORM_PENALTY = 300.0


def forms_of(pos, stem):
    """The auxiliaries `stem` (a verb stem of `pos`) takes, or None."""
    fields = pos.split(",")
    if fields[0] != "動詞" or fields[1] != "自立":
        return None
    kind, form = fields[4], fields[5]
    if form == "連用タ接続" and kind.startswith("五段"):
        if stem.endswith("ん"):
            return VOICED
        # Mozc keeps カ行イ音便 (書い) apart; the generic 五段 stems ending
        # in い are the ガ行 ones (泳い, 担い).
        if stem.endswith("い") and kind in ("五段動詞", "五段・ガ行"):
            return VOICED
        return VOICELESS
    if form == "連用形" and kind.startswith("一段"):
        return VOICELESS
    return None


def read_ours(path):
    """(reading, surface) → cost of the dictionary being re-costed."""
    ours = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            reading, surface, cost = line.rstrip("\n").split("\t")
            key = (reading, surface)
            ours[key] = min(float(cost), ours.get(key, float("inf")))
    return ours


def read_mozc(directory):
    """(reading, surface) → lowest cost, and the verb forms to add."""
    pos = {}
    with open(f"{directory}/id.def", encoding="utf-8") as f:
        for line in f:
            i, name = line.rstrip("\n").split(" ", 1)
            pos[i] = name
    costs = {}
    forms = {}
    for n in range(10):
        with open(f"{directory}/dictionary0{n}.txt", encoding="utf-8") as f:
            for line in f:
                fields = line.rstrip("\n").split("\t")
                # A sixth field marks a misspelling kept for correction.
                if len(fields) > 5:
                    continue
                reading, left, right, cost, surface = fields
                cost = int(cost)
                key = (reading, surface)
                costs[key] = min(cost, costs.get(key, cost))
                # A stem is one part of speech on both sides; 空いてる
                # (連用タ接続 on the left only) is a compound already.
                if left != right:
                    continue
                for aux in forms_of(pos[left], surface) or ():
                    form = (reading + aux, surface + aux)
                    forms[form] = min(cost, forms.get(form, cost))
    return costs, forms


def scale_map(source, target):
    """Map a cost on `source`'s scale linearly onto `target`'s, matching
    their 10th and 90th percentiles.

    Linear rather than by rank, so Mozc's order survives: Sudachi's costs
    pile up on a few values (2148 for かく's 書く, 角 and カク alike), which
    a rank mapping would fold Mozc's distinct costs onto, and reach down
    to -15000, which would put a word out of reach of any other.
    """
    source = sorted(source)
    target = sorted(target)

    def at(values, q):
        return values[round(q * (len(values) - 1))]

    slope = (at(target, 0.9) - at(target, 0.1)) / (at(source, 0.9) - at(source, 0.1))
    offset = at(target, 0.1) - slope * at(source, 0.1)
    return lambda cost: slope * cost + offset


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: mozc_costs.py dict.tsv mozc/src/data/dictionary_oss > dict.json")
    ours = read_ours(sys.argv[1])
    mozc, forms = read_mozc(sys.argv[2])

    shared = [key for key in mozc if key in ours]
    mapped = scale_map([mozc[k] for k in shared], [ours[k] for k in shared])

    merged = dict(ours)
    for key in shared:
        merged[key] = mapped(mozc[key])
    added = 0
    for key, cost in forms.items():
        if key not in merged:
            merged[key] = mapped(cost) + FORM_PENALTY
            added += 1

    by_reading = defaultdict(list)
    for (reading, surface), cost in merged.items():
        by_reading[reading].append({"surface": surface, "score": cost})
    entries = [{"reading": r, "candidates": c} for r, c in by_reading.items()]
    json.dump(entries, sys.stdout, ensure_ascii=False)
    print(
        f"{len(shared)} pairs re-costed, {added} verb forms added, "
        f"{len(merged)} pairs in all",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
