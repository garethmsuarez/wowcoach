# Season cleanup: kick tiers, avoidable damage, review pass

Goal: make the season pack's judgements match how keys are actually played, before M6 builds history on top
of them. Two gaps:

1. **Kick priority is a yes/no flag.** `priority_kick` can't say "kick Earth Bolt, but not instead of Healing
   Breeze from the same mobs". The kick plan (`rules/kickplan.rs`) covers the *busiest* spell first, and
   `missed_priority` / `wasted_kicks` (`analysis/casts.rs`) treat every tagged spell as equally urgent.
2. **No avoidable tags.** The pack has 0 `avoidable` tags, so on real keys only the uneven-hit heuristic runs
   ("Possibly avoidable"). Sites like Wipefest get this right on WCL reports because their mechanics are
   hand-curated per encounter. We can't copy that data. Instead we derive candidates by comparing our logs with
   top public keys (already cached by `season suggest --wcl-top`), and a human approves them, as with every other tag.

## What the logs support (probed 2026-11-27, 4 Den of Nalorakk keys from `test logs/`, `--dump-casts`)

| Question | Finding |
|---|---|
| How different are kick rates? | Top keys kick Healing Breeze 72/73 (99%), Earth Bolt 207/244 (85%). Other 75–87% casters: Lightning Bolt, Cinderbolt, Venom Bolt, Lava Bolt, Light Bolt, Seed Shot, Envenom, Fel Rage. |
| Do the two compete? | 32 of 182 Earth Bolt bars overlap a Healing Breeze bar. |
| Would tagging Earth Bolt `priority_kick` be fair? | No. 87 Earth Bolts went off with a ready, in-range kicker; 80 had a Healing Breeze in the same pull within 15 s, so only 7 were truly free kicks. |
| Does the group get the real priority? | Healing Breeze went off 7 of 43 times (yours), against 1 of 73 in top keys. |
| Avoidable | To be probed at the start of PR 2 (needs per-hit-event evidence that `SpellEvidence` doesn't keep yet). |

## Design

### Kick tiers
- New tag `kick` ("kick when free"), next to `priority_kick` ("must kick"). `Config::kick_tier(id)` gives
  `1` / `2` / `None`. `CastRecord` gains `tier`; `priority` stays (= tier 1), so existing code and JSON hold.
- **Wasted kicks:** kicking a tier-2 or untagged cast while a tier-1 cast goes off.
- **Free kicks** (`missed_free`): a completed tier-2 cast where the player was a ready, `in_range == Some(true)`
  kicker and no tier-1 cast in the same pull started within one of their kick cooldowns, before or after,
  whoever dealt with it (see PR 1 findings). This is how pull size is handled: in a pull with Healing Breeze
  casters, Earth Bolt misses aren't named.
- **Rules:** "Priority kicks" stays tier 1. New Low-severity player "Free kicks": aggregated per spell, at least
  3. Group kick lists put tier 1 first.
- **Kick plan:** `SpellNeed.tier`; spells sorted by tier, then rate. Tier 1 is assigned first and tier 2 gets the
  spare capacity. `uncovered` lists tier 1 only; the HTML shows tier 2 as "with spare kicks".
- **Proposals:** `season suggest` proposes `kick` for cast-bar spells that top keys kick at 70–90% of ≥ 20
  bars. `priority_kick` stays at ≥ 90%. `promote_suggestions.py` gets the same criterion.

### Avoidable evidence
- `SpellEvidence` gains per-spell hit events (hits clustered within 1 s), the events hitting ≤ 40% of the
  non-tank group, non-tank hits per player per fight, and hits landing while that spell's aura is already on the
  player (DoT ticks). Bump `EVIDENCE_VERSION`.
- Proposed `avoidable` when, in top keys: ≥ 10 hit events, ≥ 80% of them few-hit, < 30% DoT ticks, < 50% of
  damage on tanks, and top non-tank hits per player per run ≤ ½ of yours (≤ 1 with no logs of your own).
  This reads as "the mechanic happens, and good players don't get hit by it". The thresholds are confirmed by the
  PR 2 probe before they're coded.
- Tags still come from review only. The heuristic stays for spells the pack doesn't cover.

## PRs

1. **Kick tiers.** `Tag::Kick`, `kick_tier`, `CastRecord.tier`, `missed_free`, "Free kicks", kick plan order,
   `kick` proposals and the promote criterion. Tests: free kicks positive and negative (including the
   "needed for tier 1" suppression), kick plan ordering, the casts helper. Golden is re-blessed if the JSON shape changes.
2. **Avoidable evidence.** Probe first, then the evidence fields, the `avoidable` proposal, the promote
   criterion and README "Known limits".
3. **Pack review pass** (data only). Re-run `suggest --write` over all 14 local logs (2 are new since M1) and top
   keys, run `promote_suggestions.py`, and review every remaining proposal by hand, with one line per spell in the
   PR (promote / `dismissed` / leave). Add a `note` to each avoidable and tier-1 kick.
4. **Later, optional:** with `analyze --compare-top`, list spells where your group takes far more hits per player
   than the top median (per-spell hits in `RefRun`, cache `v2-…`). No tag needed.

## PR 1 findings

- **"Needed for tier 1" had to mean any must-kick nearby, not only ones the player handled.** The first version
  only excused an Earth Bolt when the player kicked (or let through) a Healing Breeze within their cooldown. Over
  the 4 Den keys that named 9–24 "free" Earth Bolts per key, often 4 players for the same bolt, and it flagged
  players holding their kick as backup for a Breeze someone else then kicked. Counting any tier-1 cast in the
  same pull within the cooldown left 1–3 free bolts per key. With Earth Bolt tagged `kick` in a scratch override,
  that gives one or two Low "Free kicks" notes in two of the four keys and none in the others.
- **Hand check** (+12 Den, 1:30): Earth Bolt started at 87.7 s and went off at 90.2 s. The Shaman's Wind Shear
  (12 s) was used on a Healing Breeze at 76.1 s, so it was back at 88.1 s, and the next Breeze started at 137.5 s.
  The Shaman is named. A Mage (Counterspell, 24 s) in range is not, since the 73.1 s Breeze is inside that window.
- **Kick plan:** Healing Breeze (~2/min) is listed before Earth Bolt (6–11/min) in every Earthwhisper Tender pack,
  and Earth Bolt is never "uncovered".
- **Proposals:** `season suggest` now proposes `priority_kick` at ≥ 90% kicked (was ≥ 50%) and `kick` at 70–90%.
  A reviewed tier is never second-guessed: a spell tagged with one tier isn't proposed the other.
  `promote_suggestions.py` drafts `kick` at 70–90% of ≥ 20 bars, and `stoppable` on either tier.
  The pack's existing `suggested` lists still say `priority_kick` for the 75–87% spells; the PR 3 re-run rewrites them.
- **Golden:** the only change is `tier: 1` on the kick plan's spell (the synthetic log uses the legacy
  `[[priority_interrupt]]` list).

## Verification
- All five CI commands; golden only re-blessed with a reviewed diff (it runs on `PackChoice::None`).
- `wowcoach season check`; `tools/promote_suggestions.py --check` → 0.
- Smoke `analyze` on the 4 Den keys before and after PR 1: Healing Breeze first in the kick plan, Earth Bolt
  only named as a free kick when no Healing Breeze was pending. Hand-check two flagged casts against raw events.
- PR 2: spot-check 3 proposed and 3 rejected avoidables against journal text and raw hits.
