# wowcoach roadmap

## Context

wowcoach already parses raw logs and public WCL reports into one `model::Event` stream, segments raid pulls and M+ runs (`analysis.rs`), and emits generic suggestions (`rules.rs`). The goal is coaching for M+ and raid, for groups and individuals. Seasonal knowledge (priority kicks, avoidables) is currently a hand-edited `wowcoach.toml`.

Decisions (2026-11-26):
- **Audience:** you and your guild. Local CLI for now; later a hosted service reachable from the guild site and Discord.
- **Season data:** pull it from the Blizzard Game Data API, then tag it by hand (assisted by evidence from the logs).
- **First focus:** M+ group play: kicks and stops, defensives on dangerous pulls, route and pacing, dispels and affixes.
- **History:** persistent, stored in SQLite.
- **WCL:** public reports, so the current client-credentials flow is enough.
- **Structure:** Cargo workspace, with the deps rule relaxed.
- **Real logs:** 9 in `test logs/` (M+, raid, mixed). Use them for validation only and never commit them.

Gaps found in the code that shape the order:
- `analysis.rs` and `rules.rs` have **no tests**. Only `combatlog.rs` (6) and `wcl.rs` (2) do.
- `AuraApplied/AuraRemoved` are parsed in both adapters but never used by analysis. This blocks work on defensives, dispels, and CC stops.
- Interrupts are counted per player only. Nothing records who kicked which cast, CC stops, or kicks per pull.
- `Config` is flat. There's no idea of a season or dungeon, and nothing selects data by `map_id` or `encounter_id`.
- `report.json` isn't gitignored (only `*.html` is).

---

## M0: Foundations (workspace + safety net)
**Status: done.** Also fixed: report output was not deterministic (sort ties broken by HashMap order). The real-log smoke run covered 12 logs (~1.5 GB, Midnight 12.1.5, log version 22): no crashes, and every player's spec was detected.
- Split into a workspace:
  - `crates/core`: model, combatlog, wcl mapping, analysis, rules, config, specs, report.
  - `crates/cli`: `main.rs`.
  - Later crates: `crates/store`, `crates/server`.
  - Network code stays behind features (`wcl`, then `blizzard`).
- Golden test: run `samples/mplus.txt` and compare the JSON against a checked-in snapshot. CLAUDE.md already treats a change in this output as a regression signal; this makes it automatic.
- Unit tests for `FightBuilder` and the rules, using small hand-built `Event` vectors. Add a test helper module for this.
- `tools/anonymise_log.py`: replace names and GUIDs consistently so that short excerpts from real logs can become fixtures.
- Real-log smoke check, run locally and not in CI: run all of `test logs/`, then fix crashes and segmentation errors (Midnight 12.x layout).
- Add `report.json` and `*.json` reports to `.gitignore`. Update CLAUDE.md and the README for the new layout.

## M1: Season data pack + Blizzard API
**Status: done.**
Detailed plan: [`docs/milestones/m1-season-data.md`](milestones/m1-season-data.md). In short:
- An embedded `data/seasons/midnight-s2.toml` pack, which can be overridden, covering the 8 tracked M+ dungeons and 3 raids. Plus a hand-curated `data/classes.toml`.
- `season sync` pulls dungeons, timers, journal encounters and spells from the Blizzard API.
- `season suggest` gets the real spell and encounter IDs from your logs and from top public keys on Warcraft Logs, and proposes tags. Journal IDs often differ from logged IDs, and trash isn't in the journal at all. WCL zone data and `dungeonPulls` supply the logged boss encounter IDs.
- Claude drafts tags from log evidence, and the user approves them in review.
- Delivered in 5 PRs: schema → sync → suggest → class tables → S2 content.

## M2: M+ kicks & stops
**Status: done.**
Detailed plan: [`docs/milestones/m2-kicks-and-stops.md`](milestones/m2-kicks-and-stops.md).
- Record how each enemy cast bar ended: completed, kicked, stopped by CC, or the caster was killed.
- Name who could have kicked a priority cast that went off: interrupt off cooldown, alive, and in range from logged positions.
- Flag wasted kicks, and suggest a kick rotation per dangerous pack.
- M+ only. Delivered in 5 PRs: data (positions, ranges, stop auras) → cast timeline → rules + report → kick plan → `stoppable` tags.

## M3: Defensives on dangerous pulls
**Status: done.**
Detailed plan: [`docs/milestones/m3-defensives.md`](milestones/m3-defensives.md).
- Consume aura events. Track personal and group defensive uptime and casts, and each defensive's availability (last use + cooldown from the `[class]` data).
- Damage-spike windows: group damage taken per X seconds above a threshold, and `tank_buster` / `dangerous_cast` hits.
- **Rules:**
  - Died with a defensive available.
  - Spike window with no group defensive.
  - Tank buster taken with no active mitigation.
  - Lust timing in M+ (current rules only cover raid).

## M4: Dispels & affixes
**Status: done.**
Detailed plan: [`docs/milestones/m4-dispels-and-affixes.md`](milestones/m4-dispels-and-affixes.md).
- Dispel latency on debuffs tagged `dispel_priority`.
- Debuffs that expired or killed someone without being dispelled.
- Who had a dispel available.
- Affix mechanics are keyed by the affix IDs from `ChallengeStart`. They're handled through pack tags, so no affix gets hard-coded.

## M5: Route & pacing
- A richer `PullReport`: enemy count and NPC IDs, duration, the trash-vs-boss time split, and time-to-kill per pack.
- Enemy forces % isn't in the log or the Blizzard API. Add an optional `npc_forces` table to the pack if wanted.
- Compare against the WCL top runs of the same dungeon and level (keystone rankings and fights over the public API): per-segment timing deltas.
- Compare against your own earlier runs once M6 lands.

## M6: Persistent history
- `crates/store` using rusqlite (bundled).
- `wowcoach ingest <log|wcl code>` stores fight summaries, per-player metrics, and suggestions, keyed by player GUID, dungeon or encounter, and date.
- `wowcoach history <player|group>`: trends and recurring suggestions ("missed the priority kick in 4 of the last 6 keys"). Add a trend section to the HTML.
- Feed history into the rules: escalate severity for repeat issues.

## M7: Raid (group + individual)
- Per-boss mechanic tags from the pack and journal.
- Raid cooldown planning vs damage windows.
- Dispel and soak execution.
- Individual: consumables, buff uptime, and comparison against WCL top parses for the spec, beyond the rank percentile.

## M8: Hosted service
- `crates/server` (axum): log upload or WCL code in, report out, backed by `store`.
- Discord: a webhook or bot that posts the run summary and links to the report.
- Privacy: guild-only auth, and retention limits on stored names and GUIDs.

---

## Verification (each milestone)
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --no-default-features`
- Golden snapshot on `samples/mplus.txt`. Extend `tools/gen_sample_log.py` with each new problem class (CC stops, unused defensive, late dispel), so every new rule has a deterministic positive case.
- Local run over `test logs/`. Check the new suggestions by hand against what actually happened in those keys and pulls.
- Network adapters (WCL, Blizzard): offline JSON fixture tests, plus one manual live run.
