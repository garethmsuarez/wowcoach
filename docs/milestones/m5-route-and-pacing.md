# M5: Route & pacing

Goal: show where a key's time went and whether the route was right. For each segment between boss kills, the plan gives:
- time and % enemy forces, against top runs of the same dungeon and level;
- the time split into combat, downtime and death penalty;
- the pulls that cost the most.

It also fixes "Timed", which today means only "completed". M+ only.

Decisions (2026-11-27):
- **Forces data:** imported from Mythic Dungeon Tools (MDT, GPL-2.0 addon data) into the pack: forces per NPC and a total per dungeon, credited to MDT.
  - `season forces --check LOGS` holds the table to the logs (a completed key must reach 100%) and proposes ID fixes for review.
  - Keystone.guru has no license and no confirmed public endpoint, so it isn't used.
- **Top runs:** `analyze --compare-top N` (opt-in, feature `wcl`) fetches the top N public runs of the key's dungeon and level and caches them. Without the flag, `analyze` stays offline.
- **Forces benchmark:** the top runs' median % forces at each boss kill, with their range. Without `--compare-top`, forces are reported but not judged against anything, except overshoot at the end.
- **Advice:** per segment, time and forces against the benchmark, split into combat, downtime and death penalty, naming the slowest pulls and their forces per minute.
- **Timed:** judged from key time (death penalties included) against the pack timer: depleted, +1, +2 (≤ 80%) or +3 (≤ 60%), with the margin ("+1, 1:12 short of +2").

## What the logs support (probed 2026-11-27, 19 keys from 13 logs, 7 public top runs)

| Question | Finding | Consequence |
|---|---|---|
| Does `CHALLENGE_MODE_END`'s flag mean timed? | No, it means completed. Field 5 is the run's rating score. Kings' Rest +13 in 1652 s scored 386.2, but in 2060 s it scored 318.5. Voidscar Arena +13: 386.0 against 319.9. **3 of your keys labelled "Timed" depleted** (Kings' Rest 2060/1980, Murder Row +7 2072/2040, Voidscar Arena 1805/1800) | Timed and upgrades come from key time against the pack timer, which the score drops confirm are right. `success` keeps meaning completed |
| Forces from MDT | The `Midnight/*.lua` files have `count` per NPC ID and `dungeonTotalCount`. Summing kills in completed keys: Murder Row 100.0–115.3%, Altar of Fangs 105.5%, Blinding Vale 102.6–102.9%, Voidscar Arena 105.8–106.0%. **Den of Nalorakk 87–90%** and **Kings' Rest 96.5%** come up short | The table needs a check against logs. *(PR 1: Den's shortfall came from the probe's regex, and Kings' Rest's Shadow of Zul is defeated at 1 HP with no death logged. See "PR 1 findings".)* |
| Kills | `UNIT_DIED` once per unique hostile GUID. Enemy counts per pull are inflated by summons: 48 Wild Imps in one Murder Row pull, 24 Unleashed Imps in another | Count kills, and keep summoned units (`SPELL_SUMMON`, or owned by another NPC) out of enemy counts. They carry 0 forces anyway |
| Top runs from WCL | `dungeonPulls` (pull times, boss kills, enemy NPCs) costs ~2–3 points per run. `enemyNPCs` instance ranges count *engaged* enemies: 109–129% at the end. Enemy death events (`dataType: Deaths, hostilityType: Enemies`, one page) plus the NPC actor map are exact: 100–104% (Kings' Rest 97%, the same MDT gap), ~2 more points per run | About 5 points per run, cached. No raw events kept: only splits, forces and pull times |
| Is the comparison meaningful? | Your +14 Murder Row: 35 / 39 / 76 / 104% forces at the four bosses, against the top +14s' 36 / 40 / 76 / 102%. **The same route, but 1850 s against about 1170 s.** Boss kills at 476 / 776 / 1358 / 1850 s against roughly 350 / 550 / 860 / 1170 s: segment 3 (Zaen → Xathuux) lost ~280 s, the others 100–180 s. One pull in segment 3 lasted 453 s | Forces at each boss separate route problems from execution problems. Then the segment breakdown (combat / downtime / deaths) and the slowest pulls say where the time went |
| Segments | Boss kills split every completed key into 3–4 segments, and top runs have the same bosses (WCL `dungeonPulls` with an encounter ID) | Segment = previous boss kill (or key start) → this boss kill |

## Design

**Pack:**
- `[[npc]]` entries: `id`, `name`, `map_id`, `forces`, `source` (`mdt`, `log`, `manual`), plus `note`.
- `forces_total` on each `[[dungeon]]`.
- Validation: the map exists, IDs are unique per map, and forces ≥ 0.
- The pack header credits MDT. Bosses and summons with 0 forces are left out.

**`season forces`** (offline, no new dependencies):
- `--mdt DIR` reads a local MDT checkout's `Midnight/*.lua` with a small purpose-built parser. It matches dungeons by name, and updates `forces` and `forces_total`. It never changes an entry whose `source` isn't `mdt`.
- `--check LOGS` gives the final forces % per completed key. Below 100%, it lists the forces NPCs never killed, and killed NPCs with 0 or unknown forces that share a name with one (the Warbringer case) as proposed aliases. With `--write` the aliases are added as `source = "log"` entries, so they show up in review.

**Analysis** (`analysis.rs`, plus a new `analysis/route.rs` that records history like the other trackers):
- `FightKind::MythicPlus` gains `timer_s`, `timed` and `upgrades` (0–3), from key time against the pack timer, or WCL's `keystoneBonus` on the WCL path. The title says "+2", "Depleted" or "Abandoned".
- `PullReport` gains:
  - `enemies` (killed, summons excluded) and `npcs` (NPC ID, name, count)
  - `forces` and `forces_pct` (with a table)
  - `combat_s`
- `FightReport.segments`, one per boss kill:
  - `boss`, `start_s`, `end_s` and the pulls in it
  - `combat_s`, `downtime_s` and `boss_s`
  - `deaths` and `death_penalty_s` (deaths × the key's penalty)
  - `forces_pct_at_end`
  - a final segment when trash is left after the last boss
- `BossMarker` gains `forces_pct`.

**Top runs** (`wcl.rs`, feature `wcl`):
- `wcl::reference(code, fight)` returns `Reference`: time, upgrades, and per boss its kill time, % forces and segment combat/downtime (from `dungeonPulls`). Built from the meta query plus enemy deaths and the pack's forces.
- Pure JSON mapping, unit-tested on a saved response. The response is trimmed to game data: NPC IDs, times, no names.
- `analyze --compare-top N`:
  - looks up the dungeon's WCL encounter (`zone_encounters`) and takes the top N public runs of the same level (`top_keys`);
  - caches each `Reference` as `~/.cache/wowcoach/compare/v1-{code}-{fight}.json`;
  - stops at `--max-points`.

  `FightReport.reference` holds the medians and ranges per boss.

**Rules** (`rules/pacing.rs`, M+ only):
- **Timer:** depleted by X / +N with X to spare / short of +N+1 by X. Levers in order of their size in this key: the death penalty total, downtime, then the slowest segment.
- **Route** (needs a reference): at each boss, your % forces against the top median. A shortfall over `forces_gap_pct` (5) means trash still to be pulled later. Over-pull at the end over `forces_over_pct` (5) is costed at the key's forces per minute ("about 1:10 of extra trash").
- **Segments** (needs a reference): segments slower than the top median by over `segment_slow_s` (60) and 20%, worst first. Each gives its split (combat / downtime / death penalty against the top runs' combat / downtime) and its slowest pulls (duration, forces per minute).
- **Without a reference:**
  - overshoot (it needs no benchmark);
  - the existing downtime rule;
  - the slowest pulls by forces per minute.
- The current "Pacing" downtime advice is folded into the segment advice when a reference exists.

**HTML:**
- Route table: segment, your split and forces, top median (range) and the delta, with combat / downtime / deaths.
- The pulls table gains enemies, forces and forces per minute.
- The run timeline marks % forces at each boss.

## PRs

1. **Data: forces in the pack.** *(done; see "PR 1 findings")* The `[[npc]]` / `forces_total` format and validation, `season forces --mdt` and `--check`.
   - Import all 8 dungeons, then run `--check` over your logs and propose the Den of Nalorakk and Kings' Rest fixes for review.
   - Credit MDT in the pack header. `CLAUDE.md` gets a note on the source and the licence.
2. **Analysis: timed fix, pulls, segments, forces.** *(done; see "PR 2 findings")* No new advice beyond the Timer fix.
   - `timed` / `upgrades`, enemies and forces per pull (summons excluded), segments, and forces at each boss.
   - Synthetic log: a summon in a pull, a key over its timer, trash after the last boss.
   - The golden snapshot stays on `PackChoice::None` (no forces). A second snapshot runs the synthetic log with a small fixture pack that has forces and a timer.
3. **Top-run reference.** `wcl::reference`, `--compare-top N` with cache and point budget, and `FightReport.reference`.
   - Offline tests on a trimmed saved response; one manual live run.
4. **Rules + report.** Timer, Route, Segments and overshoot. The HTML route table and pull columns.
   - Golden updated, then a smoke run over your logs with `--compare-top 5`. Hand-check the +14 Murder Row segment 3 claim against the raw log.

## PR 1 findings

- **Import:** `season forces --mdt DIR` imports from MDT commit `e87130bb`. All 8 dungeons have data: 145 NPCs worth forces, with totals from 551 (Ruby Life Pools) to 817 (Altar of Fangs). A second import changes nothing. The pack header credits MDT and names the commit.
- **The probe was wrong about Den of Nalorakk.** Its regex paired names, IDs and counts across neighbouring MDT entries. With a proper parser for MDT's Lua tables, Den's four completed keys reach 102–105%.
  - The 245148 Grizzled Warbringer really is a 0-forces add (606k health), not a mislabelled trash mob.
  - No ID fix is needed, and `--check` proposes none.
- **Kings' Rest's Shadow of Zul** (30 forces) is beaten to 1 HP and scripted away without a `UNIT_DIED`, about a minute before King Dazar. The check now counts a hostile unit whose last known health was ≤ 1% and that never logs a death as killed (`DEFEATED_HP_FRAC`). That moves Kings' Rest from 96.5% to 101.5% and changes no other key. PR 2's kill counting uses the same rule.
- **`season forces LOGS`** over the 13 local logs: all 18 completed keys reach 100.0% (Murder Row +2) to 115.3% (Murder Row +7, a Bargain key). No aliases are proposed and no NPC is missing.
- **Alias proposals stay for the case the check was built for:** a forces NPC that never died in a short key, and a killed same-name NPC with no forces. `--write` adds them as `source = "log"` entries with a note, for review. Unit tests cover it; your logs don't need it.

## PR 2 findings

- **Timed:** the titles of your 19 keys now match the run scores. Exactly the three keys with lower scores say "Depleted":
  - Voidscar Arena +13 (1805 s against 1800)
  - Murder Row +7 (2072 against 2040)
  - Kings' Rest +13 (2060 against 1980)

  The rest read "Timed +1" / "+2" / "+3" ("+3 The Blinding Vale" in 871 s against a 1080 s +3 timer), and the abandoned keys read "Abandoned". On the WCL path, `success` now means completed as well (`kill` or a bonus), so a depleted public run isn't mistaken for an abandoned one.
- **Forces at each boss** match the probe. The +14 Murder Row reads 35 / 39 / 76 / 104%. Segment ends equal the `ENCOUNTER_END` times to the millisecond (Kystia at 475.899 s).
- **Busy pulls are mostly forces mobs:** Demon Fly ×20 (1 forces each) and Unleashed Imp ×24 (2 each) in Murder Row are in MDT's table. Summoned units worth nothing (Spire Totem in the synthetic log) stay out of `npcs` but still count in `enemies_killed`.
- **Pulls across a boss kill:** in the +14 Murder Row, pull 9 starts in segment 3, runs through Xathuux and on into trash. It's listed in segment 3, and segment 4's combat time includes its tail. That's why `combat_s` comes from pull windows, not from pulls.
- **Synthetic log:** mobs now share one NPC ID per kind (Shade Caster 210001, Spire Guardian 210002, Spire Totem 210003), as in the game, so forces work.
  - The kick plan now sees the six Shade Caster pulls as one pack.
  - The key is completed, and a trash pull follows the last boss.
  - With no pack the title is "Completed". `tests/golden/mplus-route.json` runs the same log with `tests/fixtures/synthetic-pack.toml`, and the key is "Depleted", 34 s over a 540 s timer.

## Verification
- CI as always, with the golden snapshots (without and with forces).
- `tests/season_pack.rs`: the real-key fixture keeps passing.
- `season forces --check` over the 13 local logs:
  - every completed key at ≥ 100% after the reviewed fixes;
  - the remaining gaps explained in the PR.
- Your 19 keys:
  - timed / upgrades checked against the run score: higher score at the same level, earlier finish;
  - segment splits against the boss `ENCOUNTER_END` times;
  - forces at each boss against the probe numbers above.
- One live `--compare-top 5` run, with its point cost recorded.
