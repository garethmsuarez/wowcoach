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
| Avoidable | Probed in PR 2: see PR 2 findings. |

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
- `SpellEvidence` gains hit events (hits clustered within 1 s), the events hitting at most half the non-tanks,
  hits not aimed at the player hit (no cast of the same-named spell by that caster targeted them in the last 10 s),
  non-tank hits, and non-tank player-minutes. `EVIDENCE_VERSION` 7.
- `avoidable` is proposed from top keys only: ≥ 20 hit events, ≥ 90% of them few-hit, ≥ 70% untargeted hits,
  ≤ 1.5 non-tank hits per player per 10 minutes, < 50% of damage on tanks, and dispelled on ≤ 10% of
  applications. These replace the planned DoT exclusion and comparison with your group (see PR 2 findings).
  `promote_suggestions.py` never promotes it: every proposal is confirmed by a human.
- The uneven-hit heuristic no longer drives proposals; it stays in the report for spells the pack doesn't cover.

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

## PR 2 findings

Probed with a scratch script over the 14 local logs (all M+ keys), then 64 public top runs (5 at +16 and the
3 best per dungeon, 2,105 WCL points, cached as `v7-…` for PR 3).

- **Spread separates group-wide damage.** Of the 34 `dangerous_cast` spells, all but two hit at most half the
  non-tanks in under 65% of their hit events (those two: 73% and 82%). Candidates sit at 90–100%.
- **Targeting must match by name.** Void Beam is cast as 1300138 and does its damage as 1300156, so a match by
  ID calls every hit untargeted. With names and a 10 s window, every bolt shows 0% untargeted hits from both
  sources (Earth Bolt, Lightning Bolt, Light Bolt, Frostbolt, …).
- **DoT ticks are not excluded.** Ground puddles (Rotten Ground, Burning Steps, Fel-Scarred Earth) do their
  damage as periodic ticks from an aura on the player; the planned DoT rule would have thrown them all out.
  Puddles dropped under a targeted player still count as aimed at them and are missed (tag by hand).
- **Comparison with your group doesn't work as a test.** Per player per 10 minutes, your group gets hit by the
  candidates about as often as top keys (top ÷ yours 0.9–1.5). Only Whirling Axe (0.33) and Null Eruption (0.52)
  pass "top ≤ ½ yours". It stays in the evidence line for review.
- **How often top players get hit does work.** Random-target damage hits top players 2–25 times per player per
  10 minutes: Laced Edge 25.1, Arrow Barrage 6.7, Rotten Supplies 5.8, Siphon Energy 3.9 (which the old heuristic
  proposed). Dodgeable mechanics sit at 0.3–1.4. With ≤ 1.5 there are 17 proposals: Messy Eater, Toxic Breath,
  Echoing Fury, Rimeshatter, Earthquake, Aerial Smash, Quaking Leap, Burn Corruption, Whirling Axe, Malefic Wave,
  Flaming Embers, Fiery Demise, Tempest Winds, A Knot of Snakes, Lingering Storm, Slither Strike, Null Eruption.
  The rule (rare in top keys, rather than also requiring that your group takes more) was chosen by the user.
- **Raw-log spot checks.** Rejected: Laced Edge hits every player 40–64 times a key, mostly as a bleed; Rotten
  Supplies hits all four non-tanks 6–15 times every key; Spit Gold is a debuff on random players. Proposed: Echoing
  Fury and Whirling Axe hit unevenly (9, 4, 2, 2, 1 and 15, 12 hits), as dodged mechanics do. Earthquake is spread
  evenly (1–4 each) and applies an aura: borderline, for review.
- **Raids** get no proposals (no top-key data); the old heuristic's raid proposals (Hungering Pyre, Blood Venom, …)
  disappear on the next `--write` and should be tagged by hand.

## Verification
- All five CI commands; golden only re-blessed with a reviewed diff (it runs on `PackChoice::None`).
- `wowcoach season check`; `tools/promote_suggestions.py --check` → 0.
- Smoke `analyze` on the 4 Den keys before and after PR 1: Healing Breeze first in the kick plan, Earth Bolt
  only named as a free kick when no Healing Breeze was pending. Hand-check two flagged casts against raw events.
- PR 2: spot-check 3 proposed and 3 rejected avoidables against journal text and raw hits.
