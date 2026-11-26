# M2: M+ kicks & stops

Goal: for every enemy cast bar in a key, know how it ended and who could have
stopped it. The report then names players with evidence when a priority cast
goes off, flags wasted kicks, and suggests a kick rotation per dangerous pack.
M+ only (raid kicks are M7).

Decisions (2026-11-26):
- **Responsibility:** name players, with evidence, per event ("Healing Breeze went off at 4:12; Bravo had Pummel ready ~4 yd away").
- **Rotations:** suggest one from the logs (who should cover which priority casters).
- **What counts as a stop:** class CC (stuns, incapacitates, knockbacks, silences) landing during the cast bar, and killing the caster mid-cast. Unexplained cancels aren't credited.

## What the logs support (probed 2026-11-26, 5 keys from one log)

| Question | Finding | Consequence |
|---|---|---|
| How do cast bars end? | 892 completed, 158 kicked, 63 caster died, **47 ended silently** (no event; the mob just started its next cast) | Outcome must be inferred: no completion, kick or death, plus a stop aura during the cast = stopped |
| What stopped the silent ones? | All 47 had a group debuff on the caster during the cast. Real stops among them: Chaos Nova, Shockwave, Incapacitating Roar, Intimidation (pet), the Metamorphosis landing stun. The rest is DoT noise | Match only known stop **auras**, not any debuff |
| Are stop auras the cast IDs? | Often not: Shockwave casts 46968 but stuns as 132168. Pet Intimidation stuns as 24394 / 1258508. Some stops aren't in the class table at all | The class table needs `aura_ids` for stops, discovered and checked from logs |
| Positions? | Advanced fields carry map X/Y (yards), map ID and facing for the event's unit | "Was a kicker in range?" can be estimated from each unit's last known position |
| Reaction time | Kicks land a median 1.5 s into the cast bar (p25 1.0, p75 1.9); cast bars run ~2.5 s | Kick latency is measurable; "too late" = inside the last ~0.5 s |

## Design

**Cast timeline** (`analysis.rs`). One record per hostile cast bar:
- caster (GUID, NPC ID), spell, start/end, pull
- outcome: `Completed`, `Kicked { by, spell, latency }`, `Stopped { by, ability, cc }`, `CasterDied`, or `Cancelled` (unexplained; not credited)

Alongside it, a per-player interrupt timeline:
- every interrupt cast, including ones that hit nothing
- cooldown tracking: the class-table `cd_s`, or the player's own observed shortest reuse if that's shorter (talents, procs)
- last known position

**"Could have kicked"**, at the moment a priority cast completed:
- the player was alive
- their interrupt was off cooldown at any point during the cast bar
- their last known position was within the interrupt's range (+ a hitbox allowance) of the caster's

Position is an estimate, so the report says "~N yd". A player is only named when every check passes.

**Wasted kick:** a kick on a non-priority cast, followed within 4 s by a priority cast that went off while that player's interrupt was on cooldown.

**Kick plan.** A "pack" is the set of priority-casting NPC IDs in a pull. For each such pack across the input keys:
- casts per minute per priority spell
- how many kickers that needs, given interrupt cooldowns
- who was usually in range

From that it proposes an assignment ("Bravo and Charlie: Lightning Bolt casters; Alpha: Healing Breeze"). The assignment is greedy and covers the highest-rate casters first.

## PRs

1. **Data: positions, ranges, stop auras.** *(done; see "PR 1 findings")*
   - `AdvInfo` gains `x`, `y`, `map`, from both adapters. Check WCL's coordinate units (believed to be yards × 100) against a raw log of the same key.
   - Class table:
     - `range_yd` on interrupts
     - `aura_ids` on stops
     - missing stops added: Metamorphosis landing stun, pet Intimidation, Capacitor Totem stun via its totem
   - `classes check` gains stop discovery: harmful auras from the group on casters whose cast ended silently, that the table doesn't list.
2. **Cast timeline + interrupt availability** (analysis only, no new advice).
   - Records as above, `#[serde(skip)]` on the full list.
   - A per-pull summary table goes into the JSON.
   - Tests use synthetic event streams: kick, stun, kill, silent cancel, cooldown with observed CDR, out-of-range kicker.
3. **Rules + report.**
   - Group: priority casts that went off, **listed first** (fixes the M1 follow-up), each with "could have kicked: …".
   - Player: missed chances, wasted kicks, kick latency (median, and late kicks), kicks used vs kicks available.
   - Group: stoppable casts that went off while a stop was available (needs `stoppable` tags, PR 5).
   - HTML: a per-pull kick table with outcome per priority cast.
   - Golden snapshot updated, after extending `gen_sample_log.py` with kicks, stops and a wasted kick.
4. **Kick plan.** Pack identification, cast rates, assignment, and a "Kick plan" section per dungeon in the report. Built from all keys of that dungeon in the input (cross-week history comes with M6).
5. **`stoppable` content.**
   - `season suggest` gains stop evidence ("stopped by CC in N of M top-key casts").
   - `promote_suggestions.py` gains a `stoppable` criterion.
   - Re-run over the logs and top keys, and draft tags for review as in M1 PR 5.

## PR 1 findings

- **Positions:**
  - Raw log: X/Y/map sit at fixed offsets from the end of the advanced block (17- and 19-field layouts).
  - WCL: hundredths of a yard with the axes swapped, checked against your raw logs of the same dungeon.
  - Melee kicks measure a median of 2.9–4.4 yd on both paths, so the scale is right.
  - WCL positions are sparser, so last-known positions go stale (90th percentile Counterspell distance 67 yd on WCL vs 26 yd raw). PR 2 needs a freshness limit before naming anyone.
- **WCL adapter bug, fixed:** NPC GUIDs ignored `sourceInstance`/`targetInstance`, so every mob of one type was a single unit. On 4 public keys, "silent" cast endings dropped from 224 to 82 once fixed. `EVIDENCE_VERSION` bumped to 2, so cached `suggest` evidence is rebuilt.
- **Stops:**
  - Discovery found 18 stop auras: the aura IDs of existing stops, plus Death Grip, Frostwyrm's Fury (Absolute Zero), Havoc Metamorphosis' landing stun, Void Nova (Devourer, new in Midnight), Wake of Ashes, Rake from stealth, War Stomp, Storm Bolt and Tightening Grasp.
  - Listed stops now explain 84% of silently-ended cast bars in your keys (339/404).
  - The delve companion's Cheap Shot (Valeera) is excluded by collecting only inside keys and only from players, pets and summoned units.
  - The remaining candidate, Frostwyrm's Fury's damage debuff, rides along with the stun and isn't one.
- **Warlocks:** `classes check --wcl` on 4 public Demonology keys confirmed Spell Lock (27 kicks, now listed for every spec) and Singe Magic 132411. Axe Toss isn't confirmed yet.

## Verification
- CI as always. The golden test covers the new kick and stop scenarios in the synthetic log.
- `tests/season_pack.rs` real-key fixture: Winter's Shroud's outcome records name the kickers.
- 13 local logs, spot-checked against what happened:
  - every "could have kicked" claim on 2 keys checked by hand against the raw events
  - the kick plan for Den of Nalorakk sanity-checked with you
