#!/usr/bin/env python3
"""Draft reviewed tags from `season suggest` proposals, by explicit criteria.

Moves a tag from `suggested` to `tags` only when the spell's `evidence` line
meets the bar below (top-key numbers when present, else your own logs), and
prints a review table of what was promoted and what was left for a human:

  priority_kick    kicked >= 90% of >= 10 cast bars
  kick             kicked 70-90% of >= 20 cast bars ("kick when free": top
                   groups letting more through means they had something
                   better to kick, often a priority_kick from the same pack)
  dispel_priority  dispelled >= 70% of >= 20 applications
  tank_buster      >= 95% of damage on tanks, >= 15 casts, 0.8..5 hits per
                   cast (a hit or short combo, not a DoT or a cast that rarely
                   lands) and <= 20 casts per run (not a filler like "Shoot")
  stoppable        ended by a listed crowd-control stop >= 4 times (your logs
                   and top keys summed: whether CC works is a property of the
                   spell, not of who's playing), and only on priority_kick or
                   kick spells: that's where "a stop was ready" advice is used, and
                   filler or boss spells with surprising stop counts are left
                   for a human
  dangerous_cast   a top source of >= 5 damage spikes, >= 3 hits per landed
                   cast (group-wide, not a bolt), in a spike on >= 25% of its
                   landed casts (your logs and top keys summed), with a cast
                   bar (predictable, so a group defensive can be planned for
                   it), < 50% of its damage on tanks, and not tagged avoidable
                   (that answer is to move, not a defensive)
  affix            seen in every key with the affix in its dungeon(s), and
                   its casters in none of >= 3 keys of those dungeons
                   without it (`season suggest` asks for >= 50% and >= 2)
  avoidable        never automatically. `season suggest` proposes it from top
                   keys (rarely hit, few players per hit event, not aimed at
                   the player hit); that shows it can be dodged, not that it
                   should never hit, so every one is confirmed by a human

EXCEPTIONS lists spells that pass the numbers but are judged otherwise.
Usage: tools/promote_suggestions.py data/seasons/<pack>.toml > review.txt
       tools/promote_suggestions.py --check data/seasons/<pack>.toml
         (lists tags whose current evidence no longer meets the bar; changes nothing)
The PR that lands the result is the review: every promotion is listed there.
"""
import re, sys, tomllib

CHECK = "--check" in sys.argv[1:]
PACK = [a for a in sys.argv[1:] if a != "--check"][0]
EXCEPTIONS = {("Heartstop Poison", "tank_buster"): "a poison on the tank: the answer is a dispel, not a defensive"}

def origin_stats(ev):
    """{'yours'|'top keys': {fights, kicked, kick_n, hits, tank, disp, disp_n, instant}}"""
    out = {}
    for part in ev.split("; "):
        m = re.match(r"(yours|top keys) \((\d+) fights?\): ?(.*)", part)
        if not m: continue
        d = {"fights": int(m[2]), "kicked": 0, "kick_n": 0, "hits": 0, "tank": 0.0, "disp": 0, "disp_n": 0}
        body = m[3]
        if k := re.search(r"kicked (\d+)/(\d+)", body): d["kicked"], d["kick_n"] = int(k[1]), int(k[2])
        if h := re.search(r"(\d+) hits \((\d+)% on tanks\)", body): d["hits"], d["tank"] = int(h[1]), int(h[2]) / 100
        if x := re.search(r"dispelled (\d+)/(\d+)", body): d["disp"], d["disp_n"] = int(x[1]), int(x[2])
        d["stopped"] = int(x[1]) if (x := re.search(r"stopped by CC (\d+)", body)) else 0
        d["spikes"] = int(x[1]) if (x := re.search(r"in (\d+) damage spikes", body)) else 0
        d["instant"] = int(x[1]) if (x := re.search(r"(\d+) instant", body)) else 0
        out[m[1]] = d
    return out

def decide(name, tag, ev, tags=()):
    st = origin_stats(ev)
    d = st.get("top keys") or st.get("yours")
    src = "top keys" if "top keys" in st else "yours"
    if (name, tag) in EXCEPTIONS: return False, "exception: " + EXCEPTIONS[(name, tag)]
    if tag == "priority_kick":
        rate = d["kicked"] / d["kick_n"] if d["kick_n"] else 0
        return (d["kick_n"] >= 10 and rate >= 0.9), f"{src}: kicked {d['kicked']}/{d['kick_n']} ({rate:.0%})"
    if tag == "kick":
        rate = d["kicked"] / d["kick_n"] if d["kick_n"] else 0
        return (d["kick_n"] >= 20 and 0.7 <= rate < 0.9), f"{src}: kicked {d['kicked']}/{d['kick_n']} ({rate:.0%})"
    if tag == "dispel_priority":
        rate = d["disp"] / d["disp_n"] if d["disp_n"] else 0
        return (d["disp_n"] >= 20 and rate >= 0.7), f"{src}: dispelled {d['disp']}/{d['disp_n']} ({rate:.0%})"
    if tag == "tank_buster":
        casts = d["kick_n"]
        per_cast = d["hits"] / casts if casts else 99
        per_run = casts / d["fights"] if d["fights"] else 99
        ok = d["tank"] >= 0.95 and casts >= 15 and 0.8 <= per_cast <= 5 and per_run <= 20
        return ok, f"{src}: {casts} casts, {d['hits']} hits ({d['tank']:.0%} on tanks, {per_cast:.1f}/cast, {per_run:.1f} casts/run)"
    if tag == "stoppable":
        n = sum(o["stopped"] for o in st.values())
        if "priority_kick" not in tags and "kick" not in tags:
            return False, f"stopped by CC {n} times, but not a kick target"
        return n >= 4, f"stopped by CC {n} times (yours + top keys)"
    if tag == "dangerous_cast":
        spikes = sum(o["spikes"] for o in st.values())
        bars = sum(o["kick_n"] for o in st.values())
        landed = sum(o["kick_n"] - o["kicked"] + o["instant"] for o in st.values())
        hits = sum(o["hits"] for o in st.values())
        tank = sum(o["tank"] * o["hits"] for o in st.values()) / hits if hits else 0
        per, rate = (hits / landed, spikes / landed) if landed else (0, 0)
        why = f"in {spikes} spikes ({rate:.0%} of {landed} landed casts), {per:.1f} hits/cast, {bars} cast bars, {tank:.0%} on tanks (yours + top keys)"
        if "avoidable" in tags:
            return False, why + ", but tagged avoidable"
        return (spikes >= 5 and per >= 3 and rate >= 0.25 and bars > 0 and tank < 0.5), why
    if tag == "affix":
        m = re.search(r"affix ([\d+]+): in (\d+) of (\d+) keys? with it, its casters in none of (\d+) keys?", ev)
        if not m:
            return False, "no affix evidence"
        seen, with_, control = int(m[2]), int(m[3]), int(m[4])
        return (seen == with_ and control >= 3), f"affix {m[1]}: in {seen} of {with_} keys with it, casters absent from {control} keys without it"
    if tag == "avoidable":
        m = re.search(r"top keys \(\d+ fights?\):[^;]*?(\d+) hit events \((\d+)% few-hit, (\d+)% untargeted\), ([\d.]+) hits/player/10 min", ev)
        why = f"top keys: {m[1]} hit events, {m[2]}% few-hit, {m[3]}% untargeted, {m[4]} hits/player/10 min" if m else "no top-key hit evidence"
        return False, "for a human: " + why
    return False, "not promoted: no rule"

text = open(PACK).read()
pack = tomllib.loads(text)
inst = {d["map_id"]: d["name"] for d in pack["dungeon"] + pack["raid"]}
inst[0] = "every dungeon (affix)"

if CHECK:
    # Reviewed tags against the latest evidence. Tags set by hand without
    # evidence, and avoidable (never auto-promoted), are skipped.
    fails = []
    for s in pack["spell"]:
        ev = s.get("evidence", "")
        for t in s.get("tags", []):
            if not ev or t == "avoidable":
                continue
            ok, why = decide(s["name"], t, ev, s.get("tags", []))
            if not ok:
                fails.append(f"{t:16} | {inst.get(s['map_id'], '?')} | {s.get('boss') or 'trash'} | {s['name']} | {why}")
    print(f"{len(fails)} tag(s) no longer meet their criterion on current evidence:")
    for f in sorted(fails):
        print("  " + f)
    sys.exit(0)
head, *blocks = text.split("\n[[spell]]\n")
rows = []
new_blocks = []
for b in blocks:
    s = tomllib.loads(b)
    sug = s.get("suggested", [])
    if not sug:
        new_blocks.append(b); continue
    keep, promote = [], []
    # Judge stoppable last, so a kick tier promoted in this run counts.
    for t in sorted(sug, key=lambda t: t == "stoppable"):
        ok, why = decide(s["name"], t, s.get("evidence", ""), s.get("tags", []) + promote)
        (promote if ok else keep).append(t)
        rows.append((inst[s["map_id"]], s.get("boss") or "trash", s["name"], t, ok, why))
    tags = sorted(set(s.get("tags", []) + promote), key=["priority_kick","kick","stoppable","avoidable","dispel_priority","tank_buster","dangerous_cast","affix"].index)
    lines = [l for l in b.split("\n") if not l.startswith(("tags = ", "suggested = "))]
    i = next(i for i, l in enumerate(lines) if l.startswith("sources = ")) + 1
    ins = []
    if tags: ins.append("tags = [" + ", ".join(f'"{t}"' for t in tags) + "]")
    if keep: ins.append("suggested = [" + ", ".join(f'"{t}"' for t in keep) + "]")
    lines[i:i] = ins
    new_blocks.append("\n".join(lines))
open(PACK, "w").write("\n[[spell]]\n".join([head] + new_blocks))

for promoted in (True, False):
    print("\n## " + ("PROMOTED" if promoted else "LEFT IN suggested"))
    for r in sorted(r for r in rows if r[4] == promoted):
        print(f"{r[3]:16} | {r[0]} | {r[1]} | {r[2]} | {r[5]}")
