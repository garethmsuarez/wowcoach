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

1. **Data: dispel types, affixes, the removal/dispel pairing.** *(done; see "PR 1 findings". The removal/dispel pairing moved to PR 2, where the debuff timeline needs it)*
   - Pack:
     - `dispel = [types]` on spell entries
     - `[[affix]]`, with `death_penalty_s` (147: 15, 158/160: 5, 165: 0) and names from the Blizzard API
     - `affixes` on spell entries
     - validation in `problems()`
   - `season sync` fetches affix names. `SpellEvidence` gains `dispelled_by` (dispel ability IDs), and `EVIDENCE_VERSION` is bumped.
   - `classes check`: debuffs whose dispellers share no type, as a table error.
2. **Debuff timeline + affix-aware penalty** (analysis only, no new advice). *(done; see "PR 2 findings")*
   - Records as above, `#[serde(skip)]` on the full list. `--dump-dispels PATH` writes it for debugging.
   - Per-pull counts in `PullReport`, and the death penalty used goes into the report.
   - Synthetic tests cover:
     - removal logged before the dispel
     - dispelled, expired, died with it
     - type from the pack versus inferred
     - a dispeller on cooldown, dead, or out of range
     - an observed faster reuse
3. **Rules + report.** Players are named only when every check passes. *(done; see "PR 3 findings")*
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

## PR 1 findings

- **Affixes:** `[[affix]]` is in the pack with names from `season sync`:
  - 9 Tyrannical and 10 Fortified, with no penalty set
  - 147 Xal'atath's Guile, 15 s
  - 158 and 160 Xal'atath's Bargain: Voidbound and Devour, 5 s
  - 165 Lindormi's Guidance, 0 s

  The API has no season → affix list, so sync only names the IDs the pack lists. New IDs come from logs (PR 4).
- **Death penalty rule:** the lowest of the affixes that set one wins, so the cost of a death is never overstated. Guile and the Bargains don't occur together (Guile revokes them).
- **Spell fields:** `dispel` (debuff types) and `affixes` on spell entries. `Knowledge::dispel_types` and `death_penalty_s` read them. `SpellEvidence.dispelled_by` records the dispel abilities that removed each debuff (`EVIDENCE_VERSION` 5).
- **Soul Cleanse:** `classes check` flags exactly the probe's case. Insatiable Hunger was removed by Soul Cleanse (listed as magic) and by Remove Corruption (curse, poison). Soul Cleanse's only removal in the local logs is that debuff, and the spell API has no entry for it (new in Midnight), so its types are left as they are and the evidence is recorded in `data/classes.toml`. Open question for review.

## PR 2 findings

- **Pairing:** removal and dispel within 300 ms count as one dispel, in either order. A dispel of an open debuff closes it at once, because a test (and possibly WCL) had a dispel with no removal after it, which left the debuff open so the next application looked like a refresh.
- **Ambiguous types:** Cryo Surge is only ever removed by Purify and Cleanse, so its type is magic or disease. The first pass named a Monk's Detox as ready, but a Brewmaster or Windwalker Detox (poison, disease) can't remove a magic debuff. A dispeller now has to cover *every* candidate type.
- **Local keys** (M+ only): 73 priority debuffs dispelled (median 3.1 s) and 37 expired. Only 9 of the expiries have a known type: in keys where nobody dispelled Heartstop Poison, Cryo Surge or Corroding Spittle, the fight can't infer one. PR 4's pack `dispel` types close that gap.
- **Checked by hand:** a +12 Den of Nalorakk Cryo Surge expired on Thortic after 10 s while the Discipline priest, 7 yd away, used no dispel at all from 12:40 to 13:15.
- **Death penalty:** your keys get 15 s (Guile), 5 s (Bargain) and 0 s (Lindormi's Guidance).
- **Golden snapshot:** new fields only. The synthetic log has no priority debuffs yet (PR 3 adds them).

## PR 3 findings

- **Rules:** group "Dispels" (High when a debuff was up at a death), player "Dispel speed", and death recaps list the priority debuffs up at each death. Deaths and Timer advice use the key's penalty: 15 s on the Guile keys, 5 s on the two +7 Bargain keys, and on the 0 s keys (Lindormi's Guidance, +2 to +4) they say deaths cost no time. None of those keys had deaths, so that text only shows in tests.
- **Two filters the smoke run called for:**
  - A Cryo Surge on a +15 Den of Nalorakk dropped after 1 s with no dispel, death or immunity logged (most likely its caster died), and the first pass named a Mistweaver's Revival for it. Debuffs up under 2 s (the probe's p25 dispel time) are no longer judged.
  - Group cooldowns (dispels over 15 s: Revival, Mass Dispel) are never named for a single debuff. `ReadyDispeller` carries the table `cd_s` for this.
- **Wipe pulls:** a death with a priority debuff up only counts when the death itself is judged (`judged_deaths`, as for defensives), so the third and later deaths of a wiping pull aren't blamed on a dispel.
- **Local keys:** 5 keys get "Dispels", each naming 1 to 3 debuffs. No player reaches "Dispel speed": nobody had a median over 5 s across 3 or more priority dispels.
- **Checked by hand:** on the +14 Murder Row, Curse of Doom on Lokhos lasted from 20:09:40.6 to 20:09:50.7 (17:03 into the key) and expired. Tidereign's last Remove Corruption was at 20:09:22 (8 s cooldown) and their next at 20:09:53. The second claim (Incendiana, 20:10:03 to 20:10:13) is the same.
- **Self-dispels:** a priest's own Purify or a warlock's Singe Magic on themselves can be named (0 yd). That's correct: they could have cleared it.
- **Golden snapshot:** the synthetic key gains a magic Withering Hex (`dispel_priority` in `samples/wowcoach.toml`). It is dispelled 6 to 8 s late three times (removal logged before the dispel), expires once with the healer ready, and is up once at a judged death. The paladins' Cleanse Toxins (poison, disease) is never named.

## Verification
- CI as always. The golden test covers the new dispel scenarios in the synthetic log.
- `tests/season_pack.rs`: the real-key fixture keeps passing.
- 13 local logs, spot-checked against what happened:
  - every "expired with a dispeller ready" claim on 1 key, checked by hand against the raw events
  - the death penalty used per key, against the measured one
  - the affix entries for the two Bargain keys
