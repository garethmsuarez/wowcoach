# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Planned milestones (workspace split, Blizzard-sourced season data, M+ coaching, history, hosted service) are in `docs/ROADMAP.md`. Check it before starting feature work.

## Commands

Cargo workspace (edition 2024, needs Rust ≥ 1.85):

- `crates/core` (`wowcoach-core`, lib `wowcoach_core`): adapters, analysis, rules, config, report. Front ends call `analyze_log` / `build_report` in `lib.rs`.
- `crates/cli` (`wowcoach`, the binary): argument parsing, file I/O, the WCL fetch and the console summary.

CI (`.github/workflows/ci.yml`) runs all of these from the repo root on every PR; keep them green:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --no-default-features   # offline build without the network features (`wcl`, `blizzard`) must still compile
cargo clippy --no-default-features --all-targets -- -D warnings
```

Run one test by name filter: `cargo test combatlog::tests::timestamps` (any substring works, e.g. `cargo test wcl::`, `cargo test rules::`).

End-to-end check against the synthetic logs:

```bash
cargo run -- analyze samples/mplus.txt -c samples/wowcoach.toml -o /tmp/r.html --json /tmp/r.json
python3 tools/gen_sample_log.py raid > samples/raid.txt   # gitignored; generate on demand
```

`tools/gen_sample_log.py` is deterministic (`random.seed(7)`) and bakes in known problems (Void Pool deaths, unkicked Shadow Volley, …), so a change in analyzer output on `samples/mplus.txt` is a meaningful regression signal. The golden test enforces this (see Testing).

## Testing

- **Golden snapshot** (`crates/core/tests/golden.rs`): the full JSON report for `samples/mplus.txt` must match `crates/core/tests/golden/mplus.json`. After an intended behaviour change, run `WOWCOACH_BLESS=1 cargo test -p wowcoach-core --test golden` and review the snapshot diff in the PR.
- **Analysis unit tests** (`crates/core/src/analysis/tests.rs`) feed hand-built event streams through `Analyzer` using the builders in `crates/core/src/testutil.rs` (`Log`, `Party`, `player`, `enemy`, `analyze`).
- **Rules unit tests** (`crates/core/src/rules/tests.rs`) build `FightReport`s directly, so each test sets only the metrics a rule reads. A new rule gets a positive and a negative test here.
- **Output must be deterministic.** Report lists built from `HashMap`s need a full tie-breaker in their sort (name or spell ID), or the golden test flakes.
- **`tests/season_pack.rs`** runs the *embedded* pack on `tests/fixtures/den-of-nalorakk-excerpt.txt`, an anonymised real-key excerpt. It must keep passing as the pack evolves; if a re-tag changes it, that's a deliberate review decision.
- **Real logs** live in the gitignored `test logs/`. Use them for manual smoke runs. To turn an excerpt into a committed fixture, use `tools/anonymise_log.py LOG --list`, then `--segment N`, and write the result under `crates/core/tests/fixtures/`. The script replaces player GUIDs, names and pet names consistently and refuses to write if any original identifier remains.

## Architecture

Two sources produce one source-agnostic event stream; everything downstream exists once:

```
combatlog.rs (raw WoWCombatLog*.txt) ─┐
                                      ├─► model::Event ─► analysis::Analyzer ─► rules ─► report (HTML/JSON)
wcl.rs (Warcraft Logs v2 GraphQL) ────┘
```

All modules below live in `crates/core/src/`.

- **`model.rs`**: `Event { ts, src, dst, kind: Kind }` and `Unit` with `COMBATLOG_OBJECT_*` flag bits. `AdvInfo.pos` is a `Pos { x, y, map }` in yards, in the raw log's frame. The raw parser reads it from the end of the advanced block. WCL sends hundredths of a yard with the axes swapped (raw x = WCL y/100, raw y = −WCL x/100). `Pos::distance` returns `None` across maps. `Spell.school` is the log's school bitmask (`SCHOOL_PHYSICAL` = 1; WCL sends it as a decimal string in `masterData.abilities.type`), 0 when unknown. `ts` is in milliseconds; only differences matter. A new event type means a `Kind` variant plus producing it in **both** adapters.
- **`combatlog.rs`**: `Parser::parse_line` and `stream(reader, sink)`. The parser learns the advanced-logging field count from the log (19 in Midnight 12.x, 17 in The War Within) and finds the spec ID by position relative to the talent list. Don't hard-code field offsets.
- **`wcl.rs`**: pure JSON → `Event` mapping (`Master::from_json` / `convert`, NPC GUIDs include WCL's `sourceInstance`/`targetInstance` (one actor per NPC *type*, instances are the individual mobs; without it, a whole pack collapsed into one unit), NPC GUIDs are shaped like the game's (`Creature-0-WCL-{actor}-0-{gameID}-{instance}`) so the NPC ID is in field 5, `FightMeta` with `map_id()` from `gameZone` and `boss_events()` from `dungeonPulls`, `parse_rankings`, `parse_fight_rankings`, `zone_encounters`), unit-tested offline with `serde_json::json!`. All network code lives in `wcl::net` (ureq, OAuth client-credentials, public reports only). `rate_limit()` and `top_keys(encounter, level, page)` back `suggest --wcl-top`. M+ ranking brackets are level − 1 (+16 is bracket 15); callers still filter on the reported level. The module is behind `#[cfg(feature = "wcl")]` in `lib.rs` (the CLI's `wcl` feature forwards to `wowcoach-core/wcl`); `model.rs` carries `cfg_attr(not(feature = "wcl"), allow(dead_code))` for constants only the WCL adapter uses.
- **`analysis.rs`**: `Analyzer::feed(Event)` is a state machine. `ENCOUNTER_START/END` → raid pull; `CHALLENGE_MODE_START/END` → M+ run, where bosses inside become `BossMarker`s and pulls are split on `pull_gap_ms` combat gaps. A `FightBuilder` accumulates events and `finish()` produces a `FightReport` (players, deaths, enemy casts/spells, pulls). Fights shorter than 5s or with no players are dropped. `progression()` groups repeated pulls of the same boss and difficulty. This layer computes metrics only and makes no judgements.
- **`analysis/casts.rs`**: the cast timeline. `Tracker::feed` follows every hostile cast bar to its `CastOutcome`:
  - `Completed`, or `Kicked { by, spell, latency_s }`.
  - `Stopped { by, ability, cc }`: a class-table stop aura landed during the bar, which then ended silently (or the caster then died).
  - `CasterDied`, or `Cancelled` (unexplained, never credited).

  For completed casts of `stoppable` spells it also lists `ready_stoppers`: players with a class-table stop that they used earlier in this run, off cooldown, and in range (`range_yd`, or `STOP_DEFAULT_RANGE_YD` = 8). Stops flagged `situational` (damage cooldowns, Rake from stealth) are credited when they land but never listed as ready. For completed casts it lists `ready_kickers`: alive, with the spec's main interrupt (or an extra one they used in this fight) off cooldown by `end − REACTION_MS`. The cooldown is the class table's, or the player's own observed shortest reuse (≥ 3 s) if shorter. Distance only comes from positions fresher than `POS_FRESH_MS`; `in_range` uses `range_yd + RANGE_SLACK_YD`. `FightReport.casts` holds the records (`#[serde(skip)]`; `analyze --dump-casts PATH` writes them for debugging). `PullReport` carries per-pull counts.
- **`analysis/defensives.rs`**: defensives, spikes, tank busters, enemy summons and lust. `Tracker::feed` only records history; `finish(&Fin)` and the queries answer "at time T". It records:
  - defensive uses, from the cast or the buff, merged within 500 ms (the raw log often writes the buff first)
  - buff intervals from `aura_ids`, or `dur_s` after the cast. An `AuraRemoved` with no apply means the buff was up since before the fight.
  - group damage taken, tank-buster hits on tanks, hostile `SPELL_SUMMON`s, and Sated debuffs

  From that it reports:
  - **Ready:** a defensive counts as ready at T if it was used earlier in the fight, its cooldown (table or shortest observed reuse ≥ 3 s) has passed, and the player is alive.
  - **`Spike`:** a sliding `spike_window_ms` in which the group loses at least `spike_frac` of its summed max HP (damage after absorbs). Overlapping windows merge. Each spike lists its top spells (flagging enemy summons, with summoner and age), deaths up to 5 s after, group defensives used (from 2 s before), and group defensives ready but unused.
  - **`TankHit`:** each tank-buster hit, with the mitigation and defensives up that `protects_against` it.
  - **`EnemySummon`:** each enemy summon, with its lifetime and its damage or healing.
  - **Deaths:** `DeathReport` gains `defensives_ready` (personal, ready 1 s before) and `defensives_active`.
  - **Pull numbers:** records get theirs from `Fin.pulls` by timestamp, because the tracker is fed before `ingest` opens a new pull.
  - **Lust:** `PullReport.lust_*` is judged from up to `LUST_LEAD_MS` (10 s) before a pull, since lust goes out just before the first hit. It's available when a lust class is in the group and nobody is Sated; a lust cast with no Sated logged within 3 s counts as 10 minutes of Sated.

  The full lists are `#[serde(skip)]` on `FightReport`, and `analyze --dump-defensives PATH` writes them.
- **`rules.rs`**: all judgements live here. `apply(&mut FightReport, &Config)` runs `group_rules` and `player_rules`, which push `Suggestion { scope, severity, category, message, evidence }` built with `sugg(...)`. `scope` is `"group"` or a player name. To add a rule, add logic over `FightReport` fields; if the metric doesn't exist yet, compute it in `analysis.rs` first. `apply_progression` handles advice across pulls. **`rules/kickplan.rs`** builds `Report.kick_plans`, one per M+ dungeon across all its keys in the input. A pack is the set of priority-casting NPC IDs in a pull. Rates are casts/min over those pulls' combat time. Kickers come from the most recent key's group (main interrupt per spec), each with 60/cd kicks a minute shared across spells. Assignment is greedy with `MARGIN` = 1.25, preferring past kickers of that spell, then players in range. M+ kick advice (`priority_kicks`, plus per-player "Priority kicks" and "Wasted kicks") reads `FightReport.casts` through `analysis::missed_priority` and `analysis::wasted_kicks`, the same helpers that fill `PlayerReport.priority_missed` and `wasted_kicks`. A player is only named when `in_range == Some(true)`. **`rules/defensives.rs`** (M+ only) reads the M3 records:
  - **Player "Defensives":** deaths with `defensives_ready`. It skips wipe cascades, deaths after the first 2 of a pull with ≥ 4 deaths, and deaths with < 20% max HP taken in the last 5 s.
  - **Group "Spikes":** spikes with no group defensive used but one ready, unless an enemy summon led the spike.
  - **Group "Summons":** summons grouped by (name, summoner) that led a spike or healed enemies, when their median kill time is over `summon_alive_max_s` or none were killed.
  - **Tank "Tank busters":** at least 2 bare hits that are also ≥ 20% of the tank's tank-buster hits; it names the spec's mitigation.
  - **Group "Lust":** a `BossMarker` with `lust_ready && !lust_used`; lust idle for over `lust_idle_max_s` of combat on spiking trash; and a first-pull lust as information.
  - **`BossMarker.lust_ready/lust_used`:** judged at the boss's own start, so a boss that engages mid-pull is handled. "Used" includes a Sated debuff landing (drums).
- **`blizzard.rs`** (feature `blizzard`): `season sync`. Pure JSON mapping (`keystone_dungeon`, `journal_instance`, `encounter_spells`, `build_version`), `fetch(&dyn Api, raids, progress)` walking the Game Data API through the `Api` trait (tests use a fake serving `crates/core/tests/fixtures/blizzard/*.json`), `merge(pack, synced, wcl_encounters)` which never touches `tags`, `suggested`, `note`, logged IDs or known encounter IDs, and `render` (deterministic order, keeps the file's header comment). `blizzard::net::Client` is the only network code. Journal encounter IDs are not logged encounter IDs; raid ones come from `wcl::zone_encounters`.
- **`suggest.rs`**: `season suggest`, offline. `Collected::add(pack, fight, Origin::{Own, Top})` resolves the fight's instance (by `map_id`; by name only when the map is 0) and learns boss encounter IDs through `match_boss` (encounter ID → name → shared word → ≤2 typos, always unique or nothing). It keeps per-fight `SpellEvidence`. `FightEvidence` (title, kind, evidence) is what `suggest --wcl-top` caches per public run (`v{EVIDENCE_VERSION}-{code}-{fight}.json`); bump `EVIDENCE_VERSION` when the evidence changes. Never cache raw WCL events: they're ~70 MB per key. `apply` first maps each spell ID to a pack entry (by ID, then same name and compatible boss, else a new entry), then judges each entry **once**, on the evidence of all its IDs merged per fight, against the `MIN_*` / `*_RATE` constants. It writes `suggested` and `evidence` and never touches `tags`, `note` or `dismissed`. `analysis.rs` produces `FightReport.spell_evidence` (`#[serde(skip)]`, not in the report output). `SpellEvidence.spikes` counts the damage spikes a spell was a top source of; hits from enemy summons don't count, since they belong to the summon. `dangerous_cast` is proposed at ≥ `MIN_SPIKES` spikes, with ≥ `GROUP_HITS_PER_CAST` hits per landed cast, spikes on ≥ `SPIKE_RATE` of landed casts, and < 50% of its damage on tanks. It's judged per cast because a fight-wide "who was ever hit" share makes a bolt that rotates targets look group-wide. No rule reads the tag yet.
- **`net.rs`** (either network feature): shared `b64`, `agent()` and the OAuth `client_credentials` request. **`dotenv.rs`**: a minimal `.env` reader; the CLI loads `./.env` at startup without overriding variables already set.
- **`lib.rs`**: `analyze_log(reader, cfg)` (parse + analyse a raw log) and `build_report(source, fights, cfg)` (apply rules and progression, assemble `report::Report`).
- **`crates/cli/src/main.rs`**: CLI (`analyze LOG...`: several logs make one report, `wcl`, `init-config`, `season`, `classes`). `finish()` calls `build_report`, writes HTML/JSON and prints the console summary. On the WCL path it also attaches rankings and boss HP% to fights, matched by `start_ts`.
- **`config.rs`**: thresholds (including `spike_window_ms` / `spike_frac`), the lust/sated/healthstone/potion/absorb lists, and spell knowledge. `Config::load_with(user_toml, PackChoice)` layers the season pack (embedded / `--season-pack PATH` / none), then the user's `[[spell]]` overrides, then the older `[[avoidable]]` / `[[priority_interrupt]]` lists. Rules ask only `has_tag(id, Tag)`, `avoidable_note(id)` and `is_priority_kick(id)`. Defaults are defined **twice**: in the `DEFAULT_CONFIG` TOML string (written by `init-config`) and in `impl Default for General`. Change both together. `#[serde(default)]` means a partial user TOML is fine. `Config::default()` has no pack, which is what unit tests use.
- **`season.rs`**: `SeasonPack` (the format of `data/seasons/*.toml`: `[season]`, `[[dungeon]]`, `[[raid]]`, `[[affix]]` (`id`, `name`, `death_penalty_s`), `[[spell]]` with `ids`, `tags`, `suggested`, `dispel` (debuff types, data) and `affixes`), `problems()` validation, `EMBEDDED` (packs compiled in with `include_str!`; the last one is the default) and `Knowledge` (pack + overrides indexed by spell ID). `Tag` and `Source` are enums, so a typo is a load error. `Knowledge::dispel_types(id)` and `death_penalty_s(affixes)` read the new fields; for the death penalty, the lowest value among a key's affixes that set one wins, else `general.death_penalty_s`. `season sync` fills affix names (`keystone-affix/{id}`) only for IDs the pack lists, since the API has no season → affix list. Only `tags` drive rules; `suggested` holds tool proposals awaiting review.
- **`specs.rs`**: `SPECS` table, `spec_info(id)` → (class, spec, `Role`), `class_specs(class)`.
- **`classes.rs`**: `ClassData` (format of `data/classes.toml`: flat `[[ability]]` with `ids`, `class`, `specs`, `kind` = interrupt/stop/defensive/dispel/mitigation and kind-specific `cc` / `scope` / `types` + `target`, `cd_s`, `cdr`), validated by `problems()` and embedded with `include_str!`. `for_spec(spec_id, kind)` and `by_id(id)` are what M2–M4 rules use. `Audit` checks the table against logs: it credits `Pet-` units to their owner but never `Creature-` guardians or companions, skips dispels and `cdr` abilities in the reuse check, and only counts players of matching specs. `blizzard::check_spells` backs `classes check --live` (typed `NotFound` for 404s). Interrupts carry `range_yd`; stops carry `aura_ids` (the debuff the CC is logged as, often not the cast ID). Defensives and `mitigation` (tank active-mitigation buffs, tank specs only) carry `aura_ids` for the buff (`auras()` falls back to `ids`), `dur_s` when no buff is logged, and `school` = physical/magic when they only work against one (`protects_against(&Spell)`; unknown school counts as covered). Instant heals are `instant = true`. The audit only collects cast outcomes inside M+ keys (delve companions would pollute them). It measures kick distances from logged positions, reports debuffs removed by dispels whose listed `types` share nothing (`dispel_type_conflicts`), and finds missing stops by *lift*: auras landing during cast bars that end silently (no completion, kick or death) at least 50× more often than during completed casts. It finds missing defensive buffs the same way: buffs a player applies within 1 s after (or 200 ms before; the raw log often writes the buff first) a listed defensive cast, on ≥ 60% of its casts and ≥ 20× more often than after their casts in general. Talent procs that ride along (Matted Fur, Refreshing Drink) also pass and are left for review.

## Conventions and constraints

- Rules stay generic. Season- or tier-specific spell IDs belong in the season pack (`data/seasons/`) or the user's `wowcoach.toml`, never in code. Tools may write IDs and `suggested`, but only a human review sets `tags`.
- Season tags: `season suggest` proposes, `tools/promote_suggestions.py` drafts by fixed criteria (documented in the script), and a human approves in the PR. `--check` lists reviewed tags whose current evidence no longer meets the bar; it never removes tags. Don't hand-pick tags outside that flow without saying so in the PR.
- Keep the golden test on `PackChoice::None`: the synthetic log uses made-up spell IDs, and the snapshot must not change when the real pack grows.
- Network features: `wcl` and `blizzard` (both default, both pull in ureq). New network code goes behind one of them, with its JSON mapping testable offline from saved responses. Saved API responses must be game data only (no player names or GUIDs).
- Dependencies are deliberately minimal (anyhow, clap, serde, serde_json, toml, optional ureq), declared once in the root `[workspace.dependencies]`. Small helpers such as base64 in `wcl::net` are hand-rolled instead of pulling in a crate. `docs/ROADMAP.md` lists the planned exceptions (e.g. SQLite for history); anything else needs a reason.
- Never commit real combat logs or reports: they contain other players' names and GUIDs. Use `tools/gen_sample_log.py`, or anonymise lines for regression tests. `.gitignore` excludes `WoWCombatLog*.txt`, `test logs/`, `logs/`, `*.html`, JSON reports in the repo root and the root `wowcoach.toml`.
- Known heuristic limits (activity estimated from casts plus one GCD each, the "possibly avoidable" heuristic, ignoring wipe-cascade deaths once about a quarter of the raid is down) are documented under "Known limits" in `README.md`. Update that section when the behaviour changes.
