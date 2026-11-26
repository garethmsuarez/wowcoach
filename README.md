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
# Several logs make one report (e.g. a week of keys: the kick plan uses all of them)
./target/release/wowcoach analyze ~/WoW/_retail_/Logs/WoWCombatLog-*.txt -o week.html

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
| M+ priority kicks: each priority cast that went off, and who had a kick ready and in range | Priority casts you could have kicked; kicks wasted on filler while a priority cast went off |
| M+ kick plan per dungeon: who should cover which priority casters, from all keys of that dungeon | |
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

### Learning from logs

The journal knows boss abilities but not the IDs the game actually logs, and
nothing about trash. `wowcoach season suggest` learns them from your logs and
from top public runs on Warcraft Logs. It fills spell IDs and M+ boss encounter
IDs, and **proposes** tags with the evidence behind them:

```bash
wowcoach season suggest ~/WoW/_retail_/Logs/WoWCombatLog-*.txt             # review only
wowcoach season suggest LOGS... --wcl-top 5 --level 12..16 --write          # + top public keys, write the pack
```

Proposals land in each spell's `suggested` list with an `evidence` line, for
example `top keys (5 fights): kicked 30/30`. They never affect coaching until a
reviewer moves them to `tags`. Move a rejected one to `dismissed` and it won't
be proposed again. Public runs cost about 26 Warcraft Logs points each. Their
analysed evidence (a few KB per run, no events) is cached in
`~/.cache/wowcoach/suggest`, so re-runs are nearly free. A run stops before it
uses more than `--max-points` (default 3000) of the hourly limit.

### Class abilities

`data/classes.toml` lists each class's interrupts, crowd control that stops
casts, defensives and dispels, with cooldowns and the spell IDs they're logged
under. It's compiled in, and coaching rules use it to ask "who could have
kicked or dispelled this?" and "was a defensive available?".

```bash
wowcoach classes check                         # validate the table
wowcoach classes check --live LOGS...          # + Blizzard spell API, + audit against real logs
wowcoach classes check --wcl CODE:FIGHT        # audit against public keys (classes your logs lack)
```

The log audit fails if players used an interrupt the table doesn't list. It
also reports:
- abilities that are usually reused faster than their listed cooldown
- interrupts used from further away than their listed range
- crowd control that keeps ending enemy casts but isn't listed as a stop
- dispels that came from side effects
- abilities nobody used

The built-in Midnight Season 2 pack was built this way: from 12 guild logs and
64 top public keys. Its tags were drafted by `tools/promote_suggestions.py`
(criteria are documented in the script) and approved in review.

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

