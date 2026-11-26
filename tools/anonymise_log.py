#!/usr/bin/env python3
"""Anonymise a WoW combat log (or an excerpt of one) so it can be committed
as a regression fixture.

Every player GUID becomes Player-1-000000NN and every player name becomes
PlayerNN-Realm-US, consistently across the file (so deaths, pets, owners and
COMBATANT_INFO still line up). Player-named pets become "Pet NN". Creature
GUIDs, spell IDs and timings are kept: they're game data, not personal data.

The COMBAT_LOG_VERSION header is always kept, because the parser learns the
advanced-logging layout from it.

Examples:
    # Whole log
    tools/anonymise_log.py "test logs/WoWCombatLog-112126_151916.txt" > out.txt
    # Only the 2nd encounter / key (ENCOUNTER_START..END or CHALLENGE_MODE_START..END)
    tools/anonymise_log.py LOG --segment 2 > crates/core/tests/fixtures/boss.txt
    # List segments to pick from
    tools/anonymise_log.py LOG --list
    # Several ranges, e.g. a key's first lines (start + specs) and a later pull
    tools/anonymise_log.py LOG --lines 630362:630370 --lines 795822:796622

The output is checked for leftover original names and GUIDs before it's
written; the script exits non-zero if any remain.
"""

import argparse
import re
import sys

PLAYER_GUID = re.compile(r"Player-\d+-[0-9A-F]{8}")
# A GUID field followed by its quoted name field.
UNIT = re.compile(r'((?:Player|Pet)-[0-9A-Za-z-]+),"([^"]*)"')
SEG_START = ("ENCOUNTER_START", "CHALLENGE_MODE_START")
SEG_END = {"ENCOUNTER_START": "ENCOUNTER_END", "CHALLENGE_MODE_START": "CHALLENGE_MODE_END"}


def event_of(line):
    # "M/D/YYYY HH:MM:SS.mmm-TZ  EVENT,..." (two spaces before the event)
    try:
        rest = line.split("  ", 1)[1]
    except IndexError:
        return ""
    return rest.split(",", 1)[0]


def segments(path):
    """(start_line, end_line, event, description) for each encounter/key, 1-based inclusive."""
    out, open_seg = [], None
    with open(path, encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            ev = event_of(line)
            if ev in SEG_START:
                # A key's bosses are nested ENCOUNTER_START/ENDs: keep the key open.
                if open_seg and open_seg[2] == "CHALLENGE_MODE_START":
                    continue
                desc = line.split("  ", 1)[1].strip().split(",")
                open_seg = [n, None, ev, ",".join(desc[1:4])]
            elif open_seg and ev == SEG_END[open_seg[2]]:
                open_seg[1] = n
                out.append(tuple(open_seg))
                open_seg = None
    if open_seg:
        open_seg[1] = n
        out.append(tuple(open_seg))
    return out


def in_ranges(n, ranges):
    return any(lo <= n <= hi for lo, hi in ranges)


def build_mapping(path, ranges):
    guids, names, pets = {}, {}, {}
    last = max(hi for _, hi in ranges)
    with open(path, encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            if n > last:
                break
            if not in_ranges(n, ranges):
                continue
            for g in PLAYER_GUID.findall(line):
                guids.setdefault(g, f"Player-1-{len(guids) + 1:08X}")
            for g, name in UNIT.findall(line):
                if g.startswith("Player-") and name and name != "nil":
                    names.setdefault(name, f"Player{len(names) + 1:02d}-Realm-US")
                elif g.startswith("Pet-") and name and name != "nil":
                    pets.setdefault(name, f"Pet {len(pets) + 1:02d}")
    return guids, names, pets


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log")
    ap.add_argument("--list", action="store_true", help="list encounter/key segments and exit")
    ap.add_argument("--segment", type=int, help="only the Nth segment from --list")
    ap.add_argument("--lines", action="append", help="only lines A:B (1-based, inclusive); repeatable")
    a = ap.parse_args()

    if a.list:
        for i, (s, e, ev, desc) in enumerate(segments(a.log), 1):
            kind = "KEY " if ev == "CHALLENGE_MODE_START" else "BOSS"
            print(f"{i:3}  {kind}  lines {s}-{e} ({e - s + 1} lines)  {desc}")
        return 0

    ranges = [(1, float("inf"))]
    if a.segment:
        segs = segments(a.log)
        if not 1 <= a.segment <= len(segs):
            sys.exit(f"segment {a.segment} not found ({len(segs)} segments)")
        ranges = [segs[a.segment - 1][:2]]
    elif a.lines:
        ranges = []
        for spec in a.lines:
            x, y = spec.split(":")
            ranges.append((int(x or 1), int(y) if y else float("inf")))
    last = max(hi for _, hi in ranges)

    guids, names, pets = build_mapping(a.log, ranges)
    # Longest first so "Name-Realm-US" is replaced before any shorter overlap.
    subs = sorted({**guids, **names}.items(), key=lambda kv: -len(kv[0]))
    pattern = re.compile("|".join(re.escape(k) for k, _ in subs)) if subs else None
    table = dict(subs)

    def pet_name(m):
        g, name = m.group(1), m.group(2)
        if g.startswith("Pet-") and name in pets:
            return f'{g},"{pets[name]}"'
        return m.group(0)

    out = []
    with open(a.log, encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            if n > last:
                break
            keep = in_ranges(n, ranges) or (n == 1 and "COMBAT_LOG_VERSION" in line)
            if not keep:
                continue
            if pattern:
                line = pattern.sub(lambda m: table[m.group(0)], line)
            line = UNIT.sub(pet_name, line)
            out.append(line)

    text = "".join(out)
    leaks = [k for k in list(guids) + list(names) if k in text]
    # Pet names are only replaced on pet units (a pet called "Wolf" mustn't
    # rename the creature "Wolf"), so only check them there.
    leaks += [name for g, name in UNIT.findall(text) if g.startswith("Pet-") and name in pets]
    # Names can also appear without the region suffix (e.g. in some aura sources).
    leaks += [
        short
        for k in names
        if (short := k.rsplit("-", 1)[0]) and len(short) > 4 and f'"{short}"' in text
    ]
    if leaks:
        sys.exit(f"refusing to write: {len(leaks)} original identifier(s) still present")
    sys.stdout.write(text)
    print(
        f"anonymised {len(out)} lines: {len(guids)} player GUIDs, {len(names)} names, {len(pets)} pet names",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
