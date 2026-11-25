# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Single Rust binary crate (edition 2024, needs Rust ≥ 1.85). CI (`.github/workflows/ci.yml`) runs all of these on every PR; keep them green:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --no-default-features   # offline build without the `wcl` feature must still compile
```

Run one test by name filter: `cargo test combatlog::tests::timestamps` (any substring works, e.g. `cargo test wcl::`).

End-to-end check against the synthetic logs:

```bash
cargo run -- analyze samples/mplus.txt -c samples/wowcoach.toml -o /tmp/r.html --json /tmp/r.json
python3 tools/gen_sample_log.py raid > samples/raid.txt   # gitignored; generate on demand
```

`tools/gen_sample_log.py` is deterministic (`random.seed(7)`) and bakes in known problems (Void Pool deaths, unkicked Shadow Volley, …), so a change in analyzer output on `samples/mplus.txt` is a meaningful regression signal.

## Architecture

Two sources produce one source-agnostic event stream; everything downstream exists once:

```
combatlog.rs (raw WoWCombatLog*.txt) ─┐
                                      ├─► model::Event ─► analysis::Analyzer ─► rules ─► report (HTML/JSON)
wcl.rs (Warcraft Logs v2 GraphQL) ────┘
```

- **`model.rs`**: `Event { ts, src, dst, kind: Kind }` and `Unit` with `COMBATLOG_OBJECT_*` flag bits. `ts` is in milliseconds; only differences matter. A new event type means a `Kind` variant plus producing it in **both** adapters.
- **`combatlog.rs`**: `Parser::parse_line` and `stream(reader, sink)`. The parser learns the advanced-logging field count from the log (19 in Midnight 12.x, 17 in The War Within) and finds the spec ID by position relative to the talent list. Don't hard-code field offsets.
- **`wcl.rs`**: pure JSON → `Event` mapping (`Master::from_json` / `convert`, `FightMeta`, `parse_rankings`), unit-tested offline with `serde_json::json!`. All network code lives in `wcl::net` (ureq, OAuth client-credentials, public reports only). The module is behind `#[cfg(feature = "wcl")]` in `main.rs`; `model.rs` carries `cfg_attr(not(feature = "wcl"), allow(dead_code))` for constants only the WCL adapter uses.
- **`analysis.rs`**: `Analyzer::feed(Event)` is a state machine. `ENCOUNTER_START/END` → raid pull; `CHALLENGE_MODE_START/END` → M+ run, where bosses inside become `BossMarker`s and pulls are split on `pull_gap_ms` combat gaps. A `FightBuilder` accumulates events and `finish()` produces a `FightReport` (players, deaths, enemy casts/spells, pulls). Fights shorter than 5s or with no players are dropped. `progression()` groups repeated pulls of the same boss and difficulty. This layer computes metrics only and makes no judgements.
- **`rules.rs`**: all judgements live here. `apply(&mut FightReport, &Config)` runs `group_rules` and `player_rules`, which push `Suggestion { scope, severity, category, message, evidence }` built with `sugg(...)`. `scope` is `"group"` or a player name. To add a rule, add logic over `FightReport` fields; if the metric doesn't exist yet, compute it in `analysis.rs` first. `apply_progression` handles advice across pulls.
- **`main.rs`**: CLI (`analyze`, `wcl`, `init-config`). `finish()` applies rules, builds `report::Report`, writes HTML/JSON and prints the console summary. On the WCL path it also attaches rankings and boss HP% to fights, matched by `start_ts`.
- **`config.rs`**: thresholds and season-specific spell IDs (`[[avoidable]]`, `[[priority_interrupt]]`, and the lust/healthstone/potion/absorb lists). Defaults are defined **twice**: in the `DEFAULT_CONFIG` TOML string (written by `init-config`) and in `impl Default for General`. Change both together. `#[serde(default)]` means a partial user TOML is fine.
- **`specs.rs`**: spec ID → (class, spec, `Role`).

## Conventions and constraints

- Rules stay generic. Season- or tier-specific spell IDs belong in the user's `wowcoach.toml`, not in code.
- Dependencies are deliberately minimal (anyhow, clap, serde, serde_json, toml, optional ureq). Small helpers such as base64 in `wcl::net` are hand-rolled instead of pulling in a crate.
- Never commit real combat logs or reports: they contain other players' names and GUIDs. Use `tools/gen_sample_log.py`, or anonymise lines for regression tests. `.gitignore` excludes `WoWCombatLog*.txt`, `test logs/`, `logs/`, `*.html` and the root `wowcoach.toml`.
- Known heuristic limits (activity estimated from casts plus one GCD each, the "possibly avoidable" heuristic, ignoring wipe-cascade deaths once about a quarter of the raid is down) are documented under "Known limits" in `README.md`. Update that section when the behaviour changes.
