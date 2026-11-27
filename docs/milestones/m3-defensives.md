# M3: Defensives on dangerous pulls

Goal: for every dangerous moment in a key (a group damage spike, a tank buster, a death), know which defensives were active and which were ready but unused. The report then names players with evidence, and says when lust went unused. M+ only (raid is M7).

Decisions (2026-11-26):
- **Responsibility:** name players, with evidence, as in M2 ("Spike at 4:12, 62% of group HP in 5 s; Rallying Cry was ready (Bravo)"). A defensive only counts as ready once its player has used it in that key (most are talents or choice nodes).
- **Lust in M+:** flag a boss pull where Sated had worn off and nobody lusted, and lust left available for a long stretch of dangerous trash. A lust on the first pull is shown as information only.
- **Scope:** M+ only. Raid keeps its current rules until M7.

## What the logs support (probed 2026-11-26, 11 keys from 4 logs)

| Question | Finding | Consequence |
|---|---|---|
| Are defensives logged? | Every listed defensive shows up as `SPELL_CAST_SUCCESS`. Many also log a buff under the **same** ID (Barkskin, Ardent Defender, Die by the Sword, Survival of the Fittest). Others use a different ID or none: Blur, Greater Invisibility, Darkness, Fortifying Brew, Exhilaration and Lay on Hands (heals), and Fiery Brand (a debuff on the enemy) | The cast is the use. "Active at time T" needs `aura_ids` on defensives, discovered from logs as stops were in M2, or a listed `dur_s` when nothing is logged |
| Tank active mitigation | Self-buff uptime per tank spec: Demon Spikes 89–97%, Shuffle 71%, Ironfur 45–85%, Shield of the Righteous 69%, Ignore Pain 71–77%. Shield Block is nearly absent on Prot Warriors | Active mitigation is a **per-spec list**, not one buff. Shield Block and Ignore Pain both count for warriors |
| Tank busters (28 tagged) | Hits with active mitigation up: Vengeance 33/33, Brewmaster 25/25, Guardian 26/28, Prot Paladin 20/29, Prot Warrior **10/52** counting Shield Block only | Signal exists where active mitigation isn't near-permanent. Shield Block only helps against physical hits, so the rule needs the hit's **spell school**, which `model::Spell` doesn't carry yet |
| Spike windows | 5 s group damage taken ÷ group max HP: p50 6–25%, p99 28–79%, max 84%. Windows ≥ 50%: 0–16 per key; 16 of 60 were followed by a death within 5 s | ≥ 50% of group HP in 5 s is a usable default (configurable) |
| Group defensives in spikes | A group defensive was used within ±5 s in **7 of 60** spikes. In 28 of the remaining 53, one was ready (used earlier in the key, off cooldown) | That's the core advice: "spike with a group defensive ready" |
| Spike causes | Top spells in spike windows: Dreadbellow, Brutalize (already `tank_buster`), Searing Magma, Shadowbolt Volley, Killing Spree, Ravenous Bellow; melee in most | Cast-driven ones are candidates for `dangerous_cast`, which has **0 tags** and no `suggest` criterion today |
| Summoned units | Searing Magma isn't a cast: it comes from Magma Totems that Enthralled Shamans summon (`SPELL_SUMMON` 1246820). In the Voidscar key where it showed up in 5 spike windows, totems lived a median **10.3 s**; in the other two Voidscar keys, 3.7 s and 2.4 s, with 1–3 per key never killed. Spectral Shamans' enemy Healing Tide Totems show the same pattern | For these spikes the answer is "kill the totem", not a defensive. Spike records attribute damage to its source unit and that unit's summoner, and summon lifetimes are measured. This is generic: any hostile `SPELL_SUMMON`, no IDs in code |
| Deaths | 59 deaths. In 21 of them the player had a personal defensive they'd used earlier in the key, off cooldown; none had one active. Some hunter "deaths" took ~0% HP in their last 5 s (Feign Death or a pet mix-up) | Rule is viable. Skip deaths with negligible damage in the last 5 s, and wipe cascades |
| Lust | Lust on the first pull, then on bosses as Sated wore off. Hunter pet lust (Primal Rage) comes from the pet, which the analyzer already credits to its owner | Availability = no Sated-type debuff on the group. Tracking the debuff also catches lust from sources we don't list |

## Design

**Defensive timeline** (`analysis/defensives.rs`, beside `casts.rs` and built the same way). Per player:
- uses of each class-table defensive (the cast, or its aura when the cast wasn't seen)
- active windows (from the aura, or `dur_s` after the cast)
- cooldowns: the class-table `cd_s`, or the player's own observed shortest reuse if that's shorter, as for kicks
- ready at time T = used earlier in this key, alive, and off cooldown by T

**Active mitigation:** a new class-table kind, `mitigation`, holding the per-spec buffs. Each has an optional `school = "physical"` (Shield Block). "Covered" means one of the spec's mitigation buffs (that applies to the hit's school) or any personal or external defensive was active when the hit landed.

**Spike windows:** rolling `spike_window_ms` (5000) of damage taken by group players, at least `spike_pct` (50) of the group's summed max HP. Overlapping windows merge into one spike. Each spike records:
- start and end, and the pull
- its top spells, each with its source NPC, flagged when that source was summoned by an enemy (with the summoner and how long the summon had been alive)
- deaths during or within 5 s after
- group defensives that were active or used in it
- group defensives that were ready, with their holders

**Deaths:** `DeathReport` gains `defensives_ready` and `defensives_active`. Deaths with under 20% max HP taken in the last 5 s are skipped (Feign Death and odd units), as are wipe cascades.

**Tank busters:** per hit, the tank, the spell, its school, the mitigation active, and the defensives active.

**Enemy summons:** each hostile `SPELL_SUMMON` (totems and other adds) records its summoner, lifetime (summon → death, or still alive at pull end), and the damage it dealt to the group or the healing it did to enemies.

**Lust:** a Sated timeline from the debuff IDs in config (`sated_aura_ids`). Lust is available whenever no group player has Sated. Per pull, the analysis records whether lust was available and whether it was used.

## PRs

1. **Data: defensive auras, active mitigation, spell school.**
   - `Spell` (or the damage `Kind`s) gains `school`, from both adapters: raw log fields, and WCL `masterData.abilities.type`.
   - Class table:
     - `aura_ids` and `dur_s` on defensives
     - new kind `mitigation`: Shield of the Righteous, Shield Block (`school = physical`), Ignore Pain, Ironfur, Demon Spikes, Shuffle / Celestial Brew, Bone Shield
   - `classes check` gains defensive aura discovery: buffs a player applies within 1 s of a listed defensive cast, under a different ID, at a consistent rate.
   - Config: `sated_aura_ids`, with defaults in both `DEFAULT_CONFIG` and `impl Default`.
2. **Defensive timeline + spikes + lust state** (analysis only, no new advice).
   - Records as above, `#[serde(skip)]` on the full lists. `--dump-defensives PATH` writes them for debugging.
   - Per-pull counts go into `PullReport`.
   - Synthetic tests cover:
     - a defensive cast with and without an aura
     - observed cooldown reduction
     - a death with a defensive ready
     - a spike with and without a group defensive
     - a tank buster with mitigation, and a magic hit that Shield Block doesn't cover
     - Sated gating lust
     - an enemy totem summoned, damaging the group, and killed late
3. **Rules + report.** Players are named only when every check passes.
   - Player: "Died with a defensive ready": the ability, and when it was last used.
   - Group: "Spikes": spikes with no group defensive used while one was ready, worst first, naming the holders. When a spike's top spell comes from an enemy summon, the advice is to kill it ("Magma Totem alive 10 s, 3.1 M Searing Magma") instead of using a defensive.
   - Group: "Summons": enemy summons that dealt group damage or healed enemies and lived longer than `summon_alive_max_s` (default 6 s; your keys that handled totems well had medians of 2.4–3.7 s), or were never killed. Summons that do neither (decorative or untargetable units) are ignored.
   - Tank: "Tank busters without mitigation": count, worst spells, and what was missing.
   - Group: "Lust":
     - a boss pull where lust was available and unused
     - lust available for more than `lust_idle_max_s` (default 180) of in-combat time on pulls with a spike
     - a first-pull lust as information
   - HTML: a per-pull defensives table (spikes, tank busters, deaths with ready defensives).
   - Golden snapshot updated, after extending `gen_sample_log.py` with a spike, a defensive use and a tank buster.
4. **`dangerous_cast` content.**
   - `season suggest` gains spike evidence ("in N spike windows, M players hit per cast"). Damage from enemy summons is attributed to the summon, not proposed as a `dangerous_cast` of the caster.
   - `promote_suggestions.py` gains a `dangerous_cast` criterion.
   - Re-run over the logs and top keys, and draft tags for review, as in M2 PR 5.

## Verification
- CI as always. The golden test covers the new spike, defensive and tank-buster scenarios in the synthetic log.
- `tests/season_pack.rs`: the real-key fixture keeps passing. Add a fixture with a spike if the excerpt has none.
- 13 local logs, spot-checked against what happened:
  - every "died with a defensive ready" claim on 2 keys, checked by hand against the raw events
  - every "spike with a group defensive ready" claim on 1 key
  - the lust advice for each key, sanity-checked with you
