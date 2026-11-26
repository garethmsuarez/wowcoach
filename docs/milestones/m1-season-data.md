# M1: Season data pack + Blizzard API

Goal: season knowledge (dungeons, bosses, dangerous spells and their tags) lives in a
reviewed data pack inside the binary, instead of each user's hand-edited
`wowcoach.toml`. Rules stay generic and ask `Config` questions such as "is this a
priority kick?". M2–M4 (kicks/stops, defensives, dispels) build on the tags and
class tables added here.

Decisions (2026-11-26):
- **Coverage:** M+ dungeons plus the 3 raids in the current logs.
- **Tagging:** Claude drafts tags from log evidence and the user approves them in review. No community data sources.
- **Class data:** curated by hand in M1.
- **Distribution:** the pack is embedded in the binary and can be overridden.
- **Delivery:** 5 small PRs.
- **IDs from WCL:** Warcraft Logs (public reports + zone data) is a second source of IDs, alongside your own logs.

## What the Blizzard API actually gives (probed 2026-11-26, US, build 12.1.5)

| Endpoint | Useful content | Gaps |
|---|---|---|
| `mythic-keystone/season/index` (dynamic) | `current_season.id` = 18 | The season detail has only a name and periods: **no dungeon list** |
| `mythic-keystone/dungeon/{id}` (dynamic) | `is_tracked` (exactly the 8 current dungeons, same as the leaderboard), `map.id`, journal `dungeon.id`, `keystone_upgrades` (+1/+2/+3 timers) | — |
| `journal-instance/{id}` (static) | name, `map.id`, encounter list (journal IDs) | — |
| `journal-encounter/{id}` (static) | nested sections, with `spell {id,name}` on some | **Journal encounter ID ≠ log `ENCOUNTER_START` ID** (Taz'Rah 2791 vs 3285). No interruptible/dispel/role flags on sections |
| `journal-expansion/516`, `/505` | Midnight raids and dungeons; "Current Season" lists the M+ pool | No "current raid tier" flag (all 8 Midnight raids are listed) |
| `spell/{id}` (static) | name and description for **player** spells (Kick, Counterspell, Mind Freeze) | **404 for NPC spells**, including journal ones |
| `keystone-affix/index` | affix IDs and names | — |

Measured against a real Voidscar Arena log:
- Only 12 of 22 journal spell IDs appear in the log. The same mechanic is logged under several IDs (Nether Dash: 3 IDs, none of them the journal's).
- 59 distinct hostile trash spells, **none** in the journal.

So the journal gives structure and names, while **logs are the main source of spell IDs**.

## What Warcraft Logs adds (probed 2026-11-26)

Warcraft Logs closes the gaps the Blizzard API leaves:

| Query | Result |
|---|---|
| `worldData.expansions.zones.encounters` | Raid encounter IDs **match `ENCOUNTER_START`** (Nek'zali 3470, Entombed Sentinels 3445, Vashnik 3455, Nymrissa 3379, Kith'ix 3513). Zone 53 "The Venomous Abyss" groups all 3 instances of the tier. M+ zone 55 lists each dungeon as one encounter whose ID encodes the map ID (`12923` → 2923, `61762` → 1762, `112521` → 2521). `journalID` is always 0, so there's no journal link |
| `encounter(id).fightRankings(bracket)` | 50 ranked keys per page, about two thirds with a public `report.code` + `fightID`. A bracket filter narrows it to the group's key level |
| `report.fights { gameZone dungeonPulls { encounterID name } }` | `gameZone.id` = map ID (2923). `dungeonPulls` gives M+ boss encounter IDs (Taz'Rah 3285, Atroxus 3286, Charonus 3287) |
| Enemy spells in one public +24 Voidscar key | 76. 57 of those match IDs in our +13 log, and each side has spells the other lacks (different routes). It includes both logged Nether Dash IDs and 7 of the 10 journal spells whose IDs never matched |
| Full event fetch through the existing `wcl` path | ~42 points and ~20 s per key, against a 9000 points/hour limit, so about 200 keys an hour. Kick evidence is strong: top groups kicked every Lava Bolt, Mad Shriek and Shadowbolt Volley (0 went off) |

The existing WCL adapter has two gaps: keys come out with `map_id: 0` and no boss markers. Both can be fixed from `gameZone` and `dungeonPulls`.

**Result:** IDs come from logs, both your own and top public keys on WCL, plus the WCL zone data. Nothing is typed in by hand. Only tags need human judgement.

How logs link to the pack:
- `CHALLENGE_MODE_START` carries `instanceID` (= API `map.id`, e.g. 2813) and `challengeModeID` (= keystone dungeon ID, e.g. 587). The raw parser keeps `map_id`.
- The WCL adapter currently emits `map_id: 0` for keys, so it needs a fix or a name fallback.
- `ENCOUNTER_START` IDs come from WCL: zone encounters for raids, `dungeonPulls` for M+ bosses. Your own logs serve as a fallback. Boss entries are linked to the journal by instance + name.

## Pack format

`data/seasons/midnight-s2.toml`, embedded with `include_str!`. It's generated and merged by `season sync`, and tags are edited by hand. Output is written in a fixed order so git diffs stay clean.

```toml
[season]
key = "midnight-s2"
name = "Mythic+ Dungeons (Midnight Season 2)"
blizzard_season_id = 18
synced_build = "12.1.5"

[[dungeon]]
name = "Voidscar Arena"
keystone_id = 585        # challengeModeID in CHALLENGE_MODE_START
map_id = 2923            # instanceID in CHALLENGE_MODE_START / journal map.id
journal_id = 1313
timer_s = [1800, 1440, 1080]   # +1 / +2 / +3
bosses = [
  { name = "Taz'Rah", journal_id = 2791, encounter_id = 3285 },   # encounter_id filled from logs
]

[[raid]]
name = "The Venomous Abyss"
map_id = 3004
journal_id = 1320
bosses = [ { name = "Nek'zali the Soulcoiler", journal_id = 0, encounter_id = 3470 } ]

[[spell]]
ids = [1222098, 1296xxx]       # every ID the mechanic is logged under
name = "Nether Dash"
map_id = 2923
boss = "Taz'Rah"               # omitted = trash
sources = ["journal", "log"]
tags = ["avoidable"]           # approved tags only
suggested = []                 # from `season suggest`, not used by rules until promoted to `tags`
note = "Step out of the Shade's line"
```

Tag vocabulary (checked on load; an unknown tag is an error):
- `priority_kick`: must be interrupted.
- `stoppable`: CC (stun, incapacitate, knock) also stops it.
- `avoidable`: should never hit anyone.
- `dispel_priority`: dispel on application.
- `tank_buster`: tank should mitigate it.
- `dangerous_cast`: needs group defensives.
- `affix`: affix mechanic.

`data/classes.toml` holds expansion-level class data that doesn't change per season. Each class and spec gets:
- `interrupts`: `{id, cd_s}`
- `stops`: `{id, cd_s, kind = stun|incap|knock|silence|disorient}`
- `defensives`: `{id, cd_s, scope = personal|external|group}`
- `dispels`: `{id, cd_s, types = [magic, curse, poison, disease, bleed], target = friendly|enemy}`

## Config layering

`Config` = built-in defaults → embedded season pack → `--season-pack PATH` (replaces the embedded pack) → the user's `wowcoach.toml`.

The user file can still use `[[avoidable]]` and `[[priority_interrupt]]` (they merge as extra tags). It can also hold `[[spell]]` overrides with `ids` + `tags`, or `exclude = true` to switch off a pack entry. `--no-season-pack` turns the pack off.

Lookups are global by spell ID across the whole pack, not scoped to the current instance:
- NPC spell IDs essentially never collide.
- It works when `map_id` is unknown (the WCL path).
- `Config::avoidable_note`, `is_priority_kick` and the new `has_tag(id, tag)` keep the rules generic.

## PRs

1. **Pack schema + loader (offline).** *(done)*
   - `crates/core/src/season.rs`: types, parse, validation (unique IDs, known tags, bosses have names), the embedded pack and `Config` layering.
   - CLI flags `--season-pack` and `--no-season-pack`, and `wowcoach season check [PATH]`.
   - Ships an empty `midnight-s2.toml` skeleton.
   - Tests: layering and overrides, validation errors, an embedded-pack-is-valid test, and the golden test unchanged (the synthetic IDs don't collide).
2. **Blizzard client + `season sync`.** *(done: first live sync gave 8 dungeons, 3 raids, 38 bosses, 565 journal spells and all 10 raid encounter IDs from WCL. A second run changed nothing. Same-name journal spells are joined only within the same boss)*
   - `crates/core/src/blizzard.rs`: pure JSON → pack mapping, tested offline on API responses saved under `crates/core/tests/fixtures/blizzard/` (game data only).
   - `blizzard::net` behind a `blizzard` feature: ureq, client credentials at `oauth.battle.net`, a `--region` flag (default `us`).
   - Move the hand-rolled base64 from `wcl::net` into a shared `net` helper.
   - A small `.env` reader for `BLIZZARD_*` and `WCL_*` (no new crate); env vars take precedence.
   - `wowcoach season sync [--pack PATH] [--raid JOURNAL_ID…]`:
     - fetches the tracked dungeons with their timers, journal encounters and journal spells
     - fetches the listed raids (default: the raids already in the pack)
     - when WCL credentials are present, fills raid boss `encounter_id`s from WCL zone data, matched by name
     - merges into the pack without touching `tags`, `note`, `suggested`, log IDs or `encounter_id`
     - prints a summary of what was added and removed.
   - Run it once to create the S2 skeleton (8 dungeons; Venomous Abyss 1320, Tidebound Grotto 1317, Unbinding of Kith'ix 1324).
3. **WCL adapter fixes + `season suggest`.** *(done: over the 12 local logs, 52 fights gave 21 M+ boss encounter IDs (all bosses in dungeons the logs cover), 215 new spells, 230 logged IDs joined to existing entries and 67 proposals. Fixed on the way: an entry with several IDs is judged once on per-fight merged evidence, and no proposal comes from a single fight. Journal/log boss names that differ by a typo are matched ("Ziekket"/"Ziekett"))*
   - Fix the WCL adapter: `map_id` from fight `gameZone`, and boss markers (`EncounterStart/End` inside the key) from `dungeonPulls`. Tested offline with `json!` like the other WCL tests.
   - `wowcoach season suggest [LOG…] [--wcl CODE[:FIGHT]…] [--wcl-top N --level MIN..MAX] [--write]`:
     - Inputs are local logs, explicit WCL reports, and/or the top N public keys per pack dungeon at the given key levels (found via `fightRankings`).
     - WCL keys go through the existing `wcl::net::fetch` → `Analyzer` path, so the evidence is computed the same way for both sources.
     - Point budget: stop below a safety margin (`rateLimitData`) and cache fetched reports under the scratch/cache dir, so re-runs are free.
     - Only spell IDs, names and aggregate counts are kept. Other players' names from public reports are never written to the pack or printed.
   - For each fight whose instance is in the pack:
     - fills in boss `encounter_id` by name
     - adds log IDs to journal spells with the same name in the same instance
     - adds `[[spell]]` entries for trash and boss spells found in the logs.
   - Evidence per spell, shown separately for your groups and for top keys. "Top groups kick it 100% of the time" is a strong signal for `priority_kick`:
     - kicked / went off (already in `EnemyCastReport`)
     - non-tank hit fraction and hits per player (`EnemySpellReport`)
     - times the debuff was dispelled. This needs a new per-spell dispel count in `analysis.rs`, taken from `Dispel.extra`.
     - share of damage taken by the tank, and the biggest single hit.
   - Prints a review table. `--write` records candidates in `suggested` with an evidence note and never touches `tags`.
4. **Class tables + `classes check`.**
   - Hand-curated `data/classes.toml` for all 13 classes.
   - `wowcoach classes check` confirms every ID exists and its name matches via `spell/{id}`, which works for player spells.
   - A log check: every `SPELL_INTERRUPT` source spell seen in `test logs/` (run locally) and in the anonymised fixtures must be in the table.
   - Fill in the Midnight healing potion IDs in `potion_spell_ids` from the logs.
5. **S2 content.**
   - Run `suggest --write` over the 12 local logs plus top public keys per dungeon (e.g. `--wcl-top 5`, at your key range and at the top bracket).
   - Claude drafts `tags` from the evidence, with one line of reasoning per spell in the PR description. The user approves or edits in review.
   - Update the golden test or add a fixture so the pack is exercised on an anonymised real key.

## Out of scope for M1

- Rules that use the new tags beyond today's `avoidable` / `priority_kick`. That's M2–M4.
- Timer-based pacing. The timers are stored now; M5 uses them.
- Affix handling beyond the `affix` tag.
- Enemy forces %.

## Verification

- CI commands, including `--no-default-features` (the pack and loader must build offline; only `sync` needs the `blizzard` feature).
- PR 2: a live `season sync` against US, and a re-run produces no diff, so sync is idempotent.
- PR 3:
  - `season suggest` over `test logs/` plus `--wcl-top` fills an `encounter_id` for every M+ and raid boss in the pack and reconciles the journal IDs (e.g. Nether Dash gains its logged IDs).
  - Re-running `wowcoach wcl` on a public key shows the right `map_id` and boss markers.
  - The point cost per key is logged.
- PR 4: `classes check` passes live, and the interrupt-coverage check passes on all local logs.
- PR 5: re-run the 12-log smoke check. Kick and avoidable suggestions appear for tagged spells. Spot-check them against what actually happened in a few keys.
