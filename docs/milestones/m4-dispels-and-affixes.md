# M4: Dispels & affixes

Goal: for every priority debuff on a player, know whether it was dispelled, how fast, by whom, and who could have; and make the coaching aware of the key's affixes. M+ only (raid dispels are M7).

Decisions (2026-11-27):
- **Scope of "missed dispel":** only debuffs tagged `dispel_priority`. Every other dispellable debuff is tracked and shown, but nobody is blamed for it.
- **Responsibility:** name players, with evidence, as in M2 and M3 ("Cryo Surge on Bravo expired at 4:12; Alpha's Purify was ready, 18 yd away").
- **Slow dispels:** flag a player whose median latency on priority debuffs is over `dispel_slow_s` (5 s, configurable).
- **Affixes:** an `[[affix]]` table in the pack (names, death penalties), plus affix spell entries tagged through the usual review flow. No affix is hard-coded.

## What the logs support (probed 2026-11-27, 19 keys from 13 logs)

| Question | Finding | Consequence |
|---|---|---|
| Are friendly dispels logged? | Yes: `SPELL_DISPEL` with the dispel and the removed debuff. **But the debuff's `SPELL_AURA_REMOVED` comes first**, at the same timestamp. A naive pass counted every dispel as an expiry | Pair a removal with a dispel of the same debuff on the same player within 0.3 s, as with defensive buffs in M3 |
| Priority debuffs | 98 dispelled, 47 expired, 1 death with one up. In 43 of the 47 expiries a matching dispeller was free when the debuff landed | Rule is viable. Latency: p25 2.0 s, median 3.6 s, p75 6.4 s, p90 10.9 s |
| Debuff types | The log doesn't say whether a debuff is magic, poison, curse or disease. The types the dispellers that removed it have in common give it: Melt Armor and Essence Rend → magic, Heartstop Poison and Toxic Spores → poison, Curse of Doom → curse. Some stay ambiguous (only removed by Purify or Cleanse → magic or disease) | The pack gets a `dispel` field per spell, written by `season suggest` from that evidence (data, like IDs, not a tag). The analysis falls back to the report's own dispels |
| Class table | Insatiable Hunger was removed by Soul Cleanse (listed as magic) and Remove Corruption (curse and poison): no common type | `classes check` gains a dispel-type consistency check. Soul Cleanse's types need checking |
| Affix IDs in your keys | 9 Tyrannical, 10 Fortified, 147 Xal'atath's Guile (high keys); 158 Bargain: Voidbound and 160 Bargain: Devour (+7); 165 Lindormi's Guidance (+2–+4). Names and texts from the Blizzard API (`keystone-affix`) | Fetchable by `season sync` |
| Death penalty | Logged key time minus elapsed time, net of the ~8 s countdown, divided by deaths: **15 s** with Guile, **5 s** without (Bargain keys). Lindormi's Guidance: **0 s** ("deaths no longer reduce time remaining"; no deaths in those keys to confirm) | The rules assume 15 s everywhere today; the low-key advice "deaths cost the timer" is wrong. Per-affix `death_penalty_s` in the pack |
| Affix mechanics | Voidbound: a "Voidbound Emissary" (5761 events in one +7) casting Voidbinding and Dark Prayer. Devour: Xal'atath's "Devouring Rift" (784 events in one +7). The only spell in 3+ dungeons was a priest's own Shadow Word: Death backlash, logged with no source | Affix spells are pack entries with `map_id = 0` and `affixes = [..]`, judged by existing tags (`priority_kick`, `avoidable`…). Discovery ignores unsourced damage. Top keys are Guile-only, so Bargain data comes from your logs |

## Design

**Debuff timeline** (`analysis/dispels.rs`, recording history like `defensives.rs`). Each harmful debuff a hostile unit puts on a group player is recorded with:
- when it was applied and when it ended
- how it ended: `Dispelled { by, ability, latency_s }`, `Expired`, or `Died` (the player died with it up)
- `priority` (tagged `dispel_priority`), and the debuff's types (from the pack's `dispel`, else inferred from this report)
- for priority debuffs that weren't dispelled, `ready_dispellers`: players alive with a friendly dispel of a matching type, off cooldown for at least `REACTION_MS` while the debuff was up, and in range (40 yd, from fresh positions) when known

A friendly dispel's cooldown only starts when it removes something, so cooldowns run from `SPELL_DISPEL`, not from the cast. A player's own faster reuse counts, as for kicks.

**Per player:** dispels done, and the median and slowest latency on priority debuffs. Deaths list the priority debuffs that were up.

**Affixes:** `[[affix]]` in the pack: `id`, `name`, `death_penalty_s`, `note`. A key's death penalty is the largest one among its affixes that sets it, else `general.death_penalty_s`. Spell entries gain `affixes = [ids]` (with `map_id = 0`): they apply in every dungeon and are shown with their affix.

## PRs

1. **Data: dispel types, affixes, the removal/dispel pairing.**
   - Pack:
     - `dispel = [types]` on spell entries
     - `[[affix]]`, with `death_penalty_s` (147: 15, 158/160: 5, 165: 0) and names from the Blizzard API
     - `affixes` on spell entries
     - validation in `problems()`
   - `season sync` fetches affix names. `SpellEvidence` gains `dispelled_by` (dispel ability IDs), and `EVIDENCE_VERSION` is bumped.
   - `classes check`: debuffs whose dispellers share no type, as a table error.
2. **Debuff timeline + affix-aware penalty** (analysis only, no new advice).
   - Records as above, `#[serde(skip)]` on the full list. `--dump-dispels PATH` writes it for debugging.
   - Per-pull counts in `PullReport`, and the death penalty used goes into the report.
   - Synthetic tests cover:
     - removal logged before the dispel
     - dispelled, expired, died with it
     - type from the pack versus inferred
     - a dispeller on cooldown, dead, or out of range
     - an observed faster reuse
3. **Rules + report.** Players are named only when every check passes.
   - Group: "Dispels": priority debuffs that expired or killed someone while a dispeller was ready, worst first, naming them.
   - Player: "Dispel speed": median latency on priority debuffs over `dispel_slow_s` (at least 3 samples). Death recaps mention a priority debuff that was up.
   - The Deaths and Timer advice use the key's affix death penalty. With Lindormi's Guidance it says deaths cost no time.
   - HTML: a per-pull dispels table.
   - Golden snapshot updated, after extending `gen_sample_log.py` with a priority debuff dispelled late, one that expires with a dispeller ready, and the removal-before-dispel order.
4. **Content: dispel types, affix entries, `dispel_priority` re-check.**
   - `season suggest` writes `dispel` from `dispelled_by`.
   - It proposes affix spell entries: spells from sourced hostile units, seen in 2+ dungeons, only in keys with that affix. They get `map_id = 0`, `affixes` and the `affix` tag, alongside the usual tag proposals.
   - `promote_suggestions.py` gains an `affix` criterion.
   - Re-run over your logs and top keys, then draft for review as in M3 PR 4.

## Verification
- CI as always. The golden test covers the new dispel scenarios in the synthetic log.
- `tests/season_pack.rs`: the real-key fixture keeps passing.
- 13 local logs, spot-checked against what happened:
  - every "expired with a dispeller ready" claim on 1 key, checked by hand against the raw events
  - the death penalty used per key, against the measured one
  - the affix entries for the two Bargain keys
