# wowcoach

[![CI](https://github.com/garethmsuarez/wowcoach/actions/workflows/ci.yml/badge.svg)](https://github.com/garethmsuarez/wowcoach/actions/workflows/ci.yml)

Turns World of Warcraft combat logs into concrete improvement suggestions for
**Mythic+** and **raid**, at both the **group** and the **individual** level.

Two data sources feed one analysis pipeline:

- **Raw `WoWCombatLog*.txt`**: streaming parser, offline, ~200+ MB/s.
- **Warcraft Logs v2 API**: fetches a public report's fights and events (and per-player
  rankings for benchmarking) and maps them into the same event model.

```
raw log ──► combatlog.rs ─┐
                          ├─► model::Event ─► analysis.rs ─► rules.rs ─► report.rs (HTML/JSON)
WCL API ──► wcl.rs ───────┘     (segments,      (suggestions)
                                 metrics)
```

## Usage

```bash
cargo build --release            # add --no-default-features for an offline-only build

# Raw log (enable Advanced Combat Logging in System > Network, then /combatlog)
./target/release/wowcoach analyze ~/WoW/_retail_/Logs/WoWCombatLog-112326_201500.txt \
    -o report.html --json report.json -c wowcoach.toml

# Warcraft Logs (client from https://www.warcraftlogs.com/api/clients/)
export WCL_CLIENT_ID=... WCL_CLIENT_SECRET=...
./target/release/wowcoach wcl aBcD1234 -o report.html            # all boss/key fights
./target/release/wowcoach wcl aBcD1234 --fight 12 --fight 15

# Season/tier knowledge (avoidable spells, must-kick casts, thresholds)
./target/release/wowcoach init-config -o wowcoach.toml
```

## What it looks at

**Segmentation**: `ENCOUNTER_START/END` → raid pulls, `CHALLENGE_MODE_START/END` → M+ runs
(bosses inside a key become timeline markers). Runs are split into pulls by combat gaps.
Repeated pulls of the same boss are grouped into a **progression** summary.

| Group rules | Individual rules |
|---|---|
| Deaths and most common killing blows (+ M+ timer cost) | Death recap (last hits, HP%), avoidable vs not, healthstone/potion used |
| Raid wipe: early first death that snowballed | Avoidable damage vs group median (config-listed) |
| Missed interrupts: a cast counts as interruptible if it was kicked at least once, or is listed in config | "Possibly avoidable": took far more of a spell than peers (heuristic) |
| Avoidable damage (config) + uneven-hit heuristic | Estimated casting activity (GCD coverage while alive) |
| Bloodlust missing / late | DPS vs group DPS median with likely causes |
| M+ pacing: downtime %, longest gaps, near-wipe pulls | Healer overheal %, zero interrupts in M+ |
| Progression: recurring first-death cause, repeat deaths | WCL rank percentile (API source) |

## Season data

Spell IDs change every season, so the rules stay generic. Season knowledge
(dungeons, bosses, and enemy spells tagged `priority_kick`, `avoidable`, …)
lives in a **season pack**, `data/seasons/midnight-s2.toml`, which is compiled
into the binary:

```bash
wowcoach season check                    # summarise the built-in pack
wowcoach season check my-pack.toml       # validate a pack you're editing
wowcoach analyze LOG --season-pack my-pack.toml
wowcoach analyze LOG --no-season-pack    # only the spells in -c wowcoach.toml
```

Adjust the pack for your group in `wowcoach.toml` (see `wowcoach init-config`):

```toml
[[spell]]
ids = [1231001]
name = "Dark Swirl"
tags = ["avoidable"]
note = "Step out of the swirl before it lands"

[[spell]]            # switch a pack entry off
ids = [77777]
exclude = true
```

### Updating the pack

`wowcoach season sync` refreshes a pack from the Blizzard Game Data API: the
current keystone dungeon pool, timers, bosses and Encounter Journal spells.
Tags, notes and IDs learned from logs are never touched, and a re-run with
nothing new changes nothing. With Warcraft Logs credentials it also fills
raid bosses' logged encounter IDs.

```bash
# Credentials: a Battle.net client from https://develop.battle.net/access/clients
# (and optionally WCL_*), as env vars or in ./.env (gitignored):
#   BLIZZARD_CLIENT_ID=...
#   BLIZZARD_CLIENT_SECRET=...
wowcoach season sync --dry-run                                   # what would change
wowcoach season sync --raid 1320 --raid 1317 --raid 1324         # add raids by journal ID
wowcoach season sync                                             # refresh (raids already in the pack)
```

The older `[[avoidable]]` and `[[priority_interrupt]]` lists still work. How
packs are built from the Blizzard API, your logs and Warcraft Logs is described
in [`docs/milestones/m1-season-data.md`](docs/milestones/m1-season-data.md).

## Known limits

- **Activity** is approximated from cast events plus one GCD each, so channelled spells are
  undercounted. Compare a player against their own earlier logs, not against other specs.
- **"Possibly avoidable"** is a heuristic: targeted debuffs can trip it. Promote confirmed
  spells to `[[avoidable]]`.
- Healing is effective healing: heals minus overheal, plus healing that cleared heal-absorb
  debuffs, plus shields credited to their caster (`SPELL_ABSORBED`). Shields that only delay
  damage (Stagger, Purgatory) are excluded, and Stagger ticks don't count twice as damage taken.
  Both lists live in the config. Shields that expire unused aren't counted as overhealing.
- The raw parser learns the advanced-logging field count from the log (19 in Midnight 12.x,
  17 in The War Within) and finds the spec ID by position relative to the talent list, so
  small layout changes between patches are handled automatically.
- On raid wipes, deaths after roughly a quarter of the raid is down count as the wipe cascade
  and are ignored. Missed interrupts only count casts that showed a cast bar.
- The WCL adapter supports public reports (client-credentials flow) only. Private
  reports need the user OAuth flow.

## Development

The repo is a Cargo workspace: `crates/core` holds the parser, analysis, rules
and report; `crates/cli` is the `wowcoach` binary.

```bash
cargo test
python3 tools/gen_sample_log.py mplus > samples/mplus.txt   # synthetic logs with known problems
python3 tools/gen_sample_log.py raid  > samples/raid.txt
cargo run -- analyze samples/mplus.txt -c samples/wowcoach.toml

# After an intended change to analyzer output, refresh the golden snapshot and review the diff
WOWCOACH_BLESS=1 cargo test -p wowcoach-core --test golden

# Turn part of a real log into an anonymised test fixture
python3 tools/anonymise_log.py "test logs/WoWCombatLog-....txt" --list
python3 tools/anonymise_log.py "test logs/WoWCombatLog-....txt" --segment 2 > crates/core/tests/fixtures/boss.txt
```

Adding a rule means writing a small function over `FightReport` in
`crates/core/src/rules.rs` that pushes a `Suggestion`, plus a test in
`crates/core/src/rules/tests.rs`. Planned work is in [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Contributing

Changes go through pull requests against `main`; CI runs `cargo fmt --check`,
`cargo clippy -D warnings` and `cargo test` on Linux, macOS and Windows.

Please don't commit real combat logs or reports. They contain other players'
names and GUIDs. Use `tools/gen_sample_log.py`, or `tools/anonymise_log.py` when adding
regression tests from real logs. `.gitignore` excludes `test logs/` and `WoWCombatLog*.txt`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

