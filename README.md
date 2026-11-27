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
| Deaths and most common killing blows (+ M+ timer cost from the key's affixes) | Death recap (last hits, HP%), avoidable vs not, healthstone/potion used |
| Raid wipe: early first death that snowballed | Avoidable damage vs group median (config-listed) |
| Missed interrupts: a cast counts as interruptible if it was kicked at least once, or is listed in config | "Possibly avoidable": took far more of a spell than peers (heuristic) |
| M+ priority kicks: each priority cast that went off, and who had a kick ready and in range | Priority casts you could have kicked; kicks wasted on filler while a priority cast went off |
| M+ kick plan per dungeon: who should cover which priority casters, from all keys of that dungeon | |
| M+ stops: priority casts that crowd control also stops (`stoppable`), that went off while someone had a stun, knock or incapacitate ready and in range | |
| M+ spikes: the group losing half its HP within 5 s while a group defensive was ready and unused, naming who held it | M+: died with a personal defensive ready (used earlier in the key, off cooldown) |
| M+ summons: enemy totems and adds that led a spike or healed enemies and lived too long | M+ tanks: tank busters taken with no active mitigation or defensive up |
| M+ lust: a boss engaged with lust available and unused, lust idle through spiking trash, first-pull lust (info) | |
| M+ dispels: priority debuffs (`dispel_priority`) that expired or were up at a death while someone had a matching dispel ready and in range | M+ dispel speed: median time to dispel priority debuffs over `dispel_slow_s`; death recaps name a priority debuff that was up |
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

`suggest` also writes two kinds of data directly, since they aren't judgements:

- **Dispel types** (`dispel`): the types shared by every dispel that removed a
  debuff. They're only ever narrowed. When the dispels share no type, the debuff
  is listed for a look at the class table.
- **Affix mechanics:** a spell gets an entry with `map_id = 0` and
  `affixes = [id]` when it meets all of these:
  - it is seen only in keys with that affix, in at least half of its
    dungeon's keys with the affix;
  - the units casting it appear in no key without the affix;
  - at least 2 keys of the same dungeon without the affix are there to compare,
    and no fewer than keys with it. An affix in every high key (Xal'atath's Guile)
    would only be compared against low keys, which differ in more than the affix.

  It's proposed with the `affix` tag. A per-dungeon entry a reviewer has worked on
  (tags, note or dismissed) is never moved.

### Enemy forces

Enemy forces (the % bar) aren't in the log or any API. `season forces` imports
them from a local checkout of [Mythic Dungeon Tools](https://github.com/Nnoggie/MythicDungeonTools)
(GPL-2.0) into the pack, and checks them against your completed keys. A
completed key reached 100%, so a shortfall points at an NPC logged under an ID
the table doesn't have:

```bash
git clone --depth 1 https://github.com/Nnoggie/MythicDungeonTools /tmp/mdt
wowcoach season forces --mdt /tmp/mdt --write        # import forces per NPC
wowcoach season forces ~/WoW/_retail_/Logs/WoWCombatLog-*.txt          # check against your keys
```

A kill counts when the enemy dies, or when its last known health was 1% or
less and it never died (some enemies are scripted away at 1 HP).

### Class abilities

`data/classes.toml` lists each class's interrupts, crowd control that stops
casts, defensives, dispels and tanks' active mitigation, with cooldowns and the
spell IDs they're logged under (including buff IDs that differ from the cast). It's compiled in, and coaching rules use it to ask "who could have
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
- **Stops ready:** a stop only counts as available once its player has used it in that key
  (most are talents), and stops without a listed range are assumed to reach 8 yd, so
  ranged ones (Hammer of Justice, Storm Bolt, a hunter pet's Intimidation) are rarely
  named. Stops the log can't judge are credited when they land but never named as
  available: damage cooldowns that happen to stun (`situational` in `data/classes.toml`)
  and Rake, which only stuns from stealth.
- **Defensives ready:** like stops, a defensive only counts once its player used it in that key,
  and its cooldown is the class table's (or the player's own faster reuse, ignoring reuse under
  half the cooldown, which is a second charge). In a wiping pull only the first two deaths are
  judged, and deaths without real burst (under 20% of max HP in the last 5 s) are skipped.
- **Spikes** count damage after absorbs, so a spike a shield soaked isn't one. Spikes led by an
  enemy summon get "kill it" advice, not a defensive. A summon that expires isn't logged as
  killed, so "not killed" can mean "expired".
- **Lust** counts as available when a lust class (Shaman, Mage, Evoker, Hunter) is in the group
  and nobody is Sated; a lust cast with no Sated logged counts as 10 minutes of Sated, and a Sated
  debuff with no listed cast (drums) counts as a use. A Marksmanship hunter without a pet can't
  lust but still counts.
- **Dispels ready:** a debuff's type (magic, curse, poison, disease) isn't in the log. It comes
  from the pack's `dispel`, else from the dispels that removed it in that key; with neither,
  nobody is named. When the type is ambiguous (only ever removed by Purify: magic or disease), a
  dispeller must cover every candidate type. A healer's own dispel always counts as available;
  other dispels only once used in that key. Debuffs up under 2 s, and group cooldowns (Revival,
  Mass Dispel), are never held against anyone. A debuff that drops early because its caster
  died is logged like an expiry.
- **Timed:** the log's end-of-key flag only means the key was completed. Timed and the upgrade
  level come from the logged key time (death penalties included) against the season pack's
  timers. Without a pack, a completed key is reported as "Completed".
- **Forces and segments:** forces come from the pack's MDT-derived table, and kills from deaths
  plus enemies left at 1% health or less. A pull's kills include enemies that die just after its
  last hit. Segments split a key at boss kills; the first one includes the start countdown, and a
  pull that runs across a boss kill is listed in the segment it started in.
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

