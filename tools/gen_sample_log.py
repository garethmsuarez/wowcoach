#!/usr/bin/env python3
"""Generate synthetic WoWCombatLog files (advanced logging format) with
known problems baked in, for testing wowcoach end to end.

    python3 tools/gen_sample_log.py mplus  > samples/mplus.txt
    python3 tools/gen_sample_log.py raid   > samples/raid.txt
"""
import random
import sys
from datetime import datetime, timedelta

random.seed(7)
T0 = datetime(2026, 11, 20, 20, 0, 0)
out = []


def ts(t):
    d = T0 + timedelta(seconds=t)
    return f"{d.month}/{d.day}/{d.year} {d:%H:%M:%S}.{d.microsecond // 1000:03d}-6"


def unit(u):
    if u is None:
        return '0000000000000000,nil,0x80000000,0x80000000'
    return f'{u["guid"]},"{u["name"]}",{u["flags"]},0x0'


def adv(u, hp=None, maxhp=None):
    hp = hp if hp is not None else u.get("hp", 500000)
    maxhp = maxhp or u.get("maxhp", 500000)
    return f'{u["guid"]},{u.get("owner", "0000000000000000")},{hp},{maxhp},0,0,0,0,0,0,0,0,0,0,0,0,2000,0,90'


def line(t, ev, rest):
    out.append(f"{ts(t)}  {ev},{rest}")


def dmg(t, src, dst, sid, sname, amt, periodic=False, dst_hp=None):
    ev = "SPELL_PERIODIC_DAMAGE" if periodic else "SPELL_DAMAGE"
    if dst_hp is not None:
        dst["hp"] = max(0, dst_hp)
    line(t, ev, f'{unit(src)},{unit(dst)},{sid},"{sname}",0x20,{adv(dst)},{amt},{amt},-1,32,0,0,0,nil,nil,nil,ST')


def swing(t, src, dst, amt):
    line(t, "SWING_DAMAGE", f'{unit(src)},{unit(dst)},{adv(dst)},{amt},{amt},-1,1,0,0,0,nil,nil,nil,ST')


def heal(t, src, dst, sid, sname, amt, over):
    line(t, "SPELL_HEAL", f'{unit(src)},{unit(dst)},{sid},"{sname}",0x8,{adv(dst)},{amt},{amt},{over},0,nil')


def cast(t, src, sid, sname, start=None):
    if start is not None:
        line(start, "SPELL_CAST_START", f'{unit(src)},{unit(None)},{sid},"{sname}",0x20')
    line(t, "SPELL_CAST_SUCCESS", f'{unit(src)},{unit(None)},{sid},"{sname}",0x20,{adv(src)}')


def kick(t, src, dst, sid, sname, xid, xname):
    line(t, "SPELL_INTERRUPT", f'{unit(src)},{unit(dst)},{sid},"{sname}",0x1,{xid},"{xname}",32')


def died(t, u):
    line(t, "UNIT_DIED", f'{unit(None)},{unit(u)},0')


def combatant(t, p):
    stats = ",".join(["0"] * 21)  # Midnight: 21 stat fields
    line(t, "COMBATANT_INFO", f'{p["guid"]},0,{stats},50000,{p["spec"]},[(1,2,1)],(0,0,0,0),[(1,0,()),(2,0,())],[],[]')


def player(i, name, spec, flags):
    return {"guid": f"Player-1305-0{i:07X}", "name": f"{name}-Realm", "flags": flags, "spec": spec, "hp": 600000, "maxhp": 600000}


def npc(i, name, maxhp):
    return {"guid": f"Creature-0-1-2-3-{100000 + i}-00{i:08X}", "name": name, "flags": "0xa48", "hp": maxhp, "maxhp": maxhp}


# ---------------------------------------------------------------- M+

def mplus():
    F = "0x512"
    tank = player(1, "Shieldy", 66, F)
    heal_ = player(2, "Leafy", 105, F)
    mage = player(3, "Pyro", 63, F)
    dh = player(4, "Glaive", 577, F)
    ret = player(5, "Slacky", 70, F)
    group = [tank, heal_, mage, dh, ret]
    dps = [mage, dh, ret]
    line(0, "COMBAT_LOG_VERSION", "22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.0.5,PROJECT_ID,1")
    line(1, "CHALLENGE_MODE_START", '"Test Spire",2649,525,12,[10,9,147]')
    for p in group:
        combatant(1.1, p)

    t = 5.0
    mob_id = 0
    for pull in range(8):
        is_boss = pull in (3, 7)
        mobs = [npc(mob_id + k, "Spire Guardian" if is_boss else "Shade Caster", 30_000_000 if is_boss else 4_000_000) for k in range(1 if is_boss else 4)]
        mob_id += 5
        if is_boss:
            line(t, "ENCOUNTER_START", f'{3000 + pull},"Spire Boss {pull}",8,5,2649')
        dur = 75 if is_boss else 40
        if pull == 0:
            cast(t, mage, 80353, "Time Warp")
        dead = set()
        s = 0.0
        while s < dur:
            now = t + s
            for p in group:
                if p["name"] in dead:
                    continue
                tgt = random.choice(mobs)
                gap = 1.2 if p is not ret else (1.2 if random.random() < 0.55 else 3.5)
                if (s * 10) % round(gap * 10) < 1e-6 or random.random() < 0.6 / gap:
                    cast(now, p, 100 + p["spec"], "Rotation Spell")
                    if p is not heal_:
                        amount = {66: 70000, 63: 160000, 577: 150000, 70: 110000}[p["spec"]]
                        dmg(now + 0.05, p, tgt, 100 + p["spec"], "Rotation Spell", int(amount * random.uniform(0.8, 1.2)))
                    else:
                        who = random.choice(group)
                        heal(now + 0.05, heal_, who, 8936, "Regrowth", 90000, random.choice([0, 0, 40000, 90000]))
            for m in mobs:
                swing(now + 0.1, m, tank, 45000)
            # caster volley every 8s, sometimes kicked
            if not is_boss and int(s) % 8 == 0 and abs(s - int(s)) < 1e-6:
                caster = mobs[0]
                line(now, "SPELL_CAST_START", f'{unit(caster)},{unit(None)},77777,"Shadow Volley",0x20')
                if random.random() < 0.45:
                    kicker = random.choice([tank, dh])  # mage and ret never kick
                    kick(now + 1.0, kicker, caster, 96231, "Rebuke", 77777, "Shadow Volley")
                else:
                    line(now + 2.0, "SPELL_CAST_SUCCESS", f'{unit(caster)},{unit(None)},77777,"Shadow Volley",0x20,{adv(caster)}')
                    for p in group:
                        if p["name"] not in dead:
                            dmg(now + 2.1, caster, p, 77777, "Shadow Volley", 120000)
            # void pools: mage stands in them, others mostly don't
            if int(s) % 5 == 0 and abs(s - int(s)) < 1e-6:
                for p in dps + [heal_]:
                    if p["name"] in dead:
                        continue
                    if p is mage or random.random() < 0.06:
                        hp = p["hp"] - 140000
                        dmg(now + 0.3, mobs[0], p, 88888, "Void Pool", 140000, periodic=True, dst_hp=hp)
                        if p["hp"] <= 0 and p["name"] not in dead:
                            died(now + 0.4, p)
                            dead.add(p["name"])
                    else:
                        p["hp"] = min(p["maxhp"], p["hp"] + 100000)
            # pull 5: overpull, three people die to a big hit
            if pull == 5 and abs(s - 20.0) < 1e-6:
                for p in (heal_, dh, ret):
                    dmg(now, mobs[1], p, 99999, "Cataclysmic Slam", 900000, dst_hp=0)
                    died(now + 0.1, p)
                    dead.add(p["name"])
            s = round(s + 0.5, 2)
        for m in mobs:
            died(t + dur, m)
        for p in group:
            p["hp"] = p["maxhp"]
            if p["name"] in dead:
                cast(t + dur + 3, p, 100 + p["spec"], "Rotation Spell")  # resurrected
        if is_boss:
            line(t + dur, "ENCOUNTER_END", f'{3000 + pull},"Spire Boss {pull}",8,5,1,{int(dur * 1000)}')
        t += dur + (55 if pull == 4 else 15)  # one very long break
    line(t, "CHALLENGE_MODE_END", f"2649,0,12,{int((t - 1) * 1000)},0.0,0.0")


# ---------------------------------------------------------------- raid

def raid():
    F = "0x514"
    specs = [66, 250, 105, 264, 257, 63, 577, 70, 62, 72, 254, 258, 102, 1467, 266, 251, 259, 269, 263, 103]
    names = ["Wall", "Bloody", "Treebro", "Totem", "Halo", "Pyro", "Glaive", "Slacky", "Arcy", "Rage",
             "Arrow", "Shadow", "Moonkin", "Scales", "Demo", "Frosty", "Stabby", "Fist", "Enh", "Kitty"]
    group = [player(i + 1, n, s, F) for i, (n, s) in enumerate(zip(names, specs))]
    line(0, "COMBAT_LOG_VERSION", "22,ADVANCED_LOG_ENABLED,1,BUILD_VERSION,12.0.5,PROJECT_ID,1")
    t = 10.0
    for attempt, (dur, kill) in enumerate([(140, False), (190, False), (260, True)]):
        boss = npc(900, "Dimensius", 2_000_000_000)
        line(t, "ENCOUNTER_START", '3135,"Dimensius",16,20,2810')
        for p in group:
            p["hp"] = p["maxhp"]
            combatant(t + 0.1, p)
        if attempt == 2:
            cast(t + 1, group[5], 80353, "Time Warp")
        total = boss["maxhp"]
        dmg_per_s = total / 255 * (0.92 if not kill else 1.0)
        dead = set()
        s = 0.0
        while s < dur:
            now = t + s
            alive = [p for p in group if p["name"] not in dead]
            for p in alive:
                role_heal = p["spec"] in (105, 264, 257)
                if p["name"].startswith("Slacky") and random.random() < 0.5:
                    pass  # idle
                else:
                    cast(now, p, 100 + p["spec"], "Rotation Spell")
                    if role_heal:
                        heal(now + 0.05, p, random.choice(alive), 2060, "Heal", 200000, random.choice([0, 50000, 150000]))
                    else:
                        amt = int(dmg_per_s / 17 * random.uniform(0.8, 1.2) * (0.5 if p["name"].startswith("Slacky") else 1))
                        boss["hp"] = max(1, boss["hp"] - amt)
                        dmg(now + 0.05, p, boss, 100 + p["spec"], "Rotation Spell", amt, dst_hp=boss["hp"])
            swing(now + 0.2, boss, group[0], 300000)
            # Dark Swirl: avoidable; "Rage" and "Arcy" keep eating it, and first deaths come from it
            if int(s) % 6 == 0 and abs(s - int(s)) < 1e-6:
                for p in alive:
                    if p["spec"] in (66, 250):
                        continue
                    sloppy = p["name"] in ("Rage-Realm", "Arcy-Realm")
                    if (sloppy and random.random() < 0.7) or random.random() < 0.05:
                        p["hp"] -= 260000 if not kill else 150000
                        dmg(now + 0.3, boss, p, 1231001, "Dark Swirl", 260000, dst_hp=p["hp"])
                        if p["hp"] <= 0 and p["name"] not in dead:
                            died(now + 0.35, p)
                            dead.add(p["name"])
                    else:
                        p["hp"] = min(p["maxhp"], p["hp"] + 90000)
            # raid-wide pulse every 20s (unavoidable)
            if int(s) % 20 == 0 and abs(s - int(s)) < 1e-6:
                for p in alive:
                    dmg(now + 0.5, boss, p, 1231500, "Gravity Pulse", 180000)
            s = round(s + 1.0, 2)
        line(t + dur, "ENCOUNTER_END", f'3135,"Dimensius",16,20,{1 if kill else 0},{dur * 1000}')
        t += dur + 120


{"mplus": mplus, "raid": raid}[sys.argv[1]]()
print("\n".join(out))
