//! Kick plan: a suggested interrupt assignment for each dangerous M+ pack,
//! from all the keys of a dungeon in the input.
//!
//! A "pack" is the set of NPC types that cast priority spells in a pull, so
//! the same pack is recognised across keys even when the route differs.
//! For each pack we measure how often each priority spell is cast, then
//! assign kickers greedily: must-kicks (`priority_kick`) first, busiest first,
//! then kick-when-free spells (`kick`) with the spare capacity. Each kicker
//! can do 60 / cooldown kicks a minute (shared across the spells they cover), with
//! a margin for travel and reaction time. Players who already kicked a spell,
//! then players who were usually in range of it, are preferred.

use crate::analysis::{CastOutcome, CastRecord, FightKind, FightReport};
use crate::classes::ClassData;
use crate::specs::SPECS;
use serde::Serialize;
use std::collections::BTreeMap;

/// Assign this much more kick capacity than the observed cast rate.
pub const MARGIN: f64 = 1.25;
/// Packs listed per dungeon.
pub const MAX_PACKS: usize = 10;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SpellNeed {
    pub spell: String,
    /// 1 = must kick (`priority_kick`), 2 = kick when free (`kick`).
    pub tier: u8,
    pub casts: u32,
    pub casts_per_min: f64,
    pub kicked: u32,
    pub stopped: u32,
    pub went_off: u32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Assignment {
    pub player: String,
    pub ability: String,
    pub spells: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PackPlan {
    /// Names of the priority casters in the pack.
    pub name: String,
    pub npc_ids: Vec<u32>,
    /// Pulls of this pack across the input keys.
    pub pulls: u32,
    pub spells: Vec<SpellNeed>,
    pub assignments: Vec<Assignment>,
    /// Must-kicks the group's interrupts can't cover at the observed rate
    /// (kick-when-free spells only get spare capacity, so never count).
    pub uncovered: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct KickPlan {
    pub dungeon: String,
    pub keys: u32,
    /// The group the plan is for (the most recent key of the dungeon).
    pub group: Vec<String>,
    pub packs: Vec<PackPlan>,
}

#[derive(Default)]
struct PackAcc {
    names: BTreeMap<u32, String>,
    pulls: u32,
    minutes: f64,
    spells: BTreeMap<String, SpellNeed>,
    /// (player, spell) -> times they kicked it / were ready and in range.
    kicked_by: BTreeMap<(String, String), u32>,
    in_range: BTreeMap<(String, String), u32>,
}

/// A kicker in the current group: name, interrupt, cooldown in seconds.
struct Kicker {
    name: String,
    ability: String,
    capacity: f64,
}

/// One plan per dungeon with M+ keys in `fights`.
pub fn kick_plans(fights: &[FightReport]) -> Vec<KickPlan> {
    let mut by_dungeon: BTreeMap<String, Vec<&FightReport>> = BTreeMap::new();
    for f in fights {
        if let FightKind::MythicPlus { zone, .. } = &f.kind {
            by_dungeon.entry(zone.clone()).or_default().push(f);
        }
    }
    let mut out = vec![];
    for (dungeon, keys) in by_dungeon {
        let mut packs: BTreeMap<Vec<u32>, PackAcc> = BTreeMap::new();
        for f in &keys {
            let mut per_pull: BTreeMap<usize, Vec<&CastRecord>> = BTreeMap::new();
            for c in f.casts.iter().filter(|c| c.tier.is_some()) {
                if let Some(p) = c.pull {
                    per_pull.entry(p).or_default().push(c);
                }
            }
            for (pull, casts) in per_pull {
                let mut ids: Vec<u32> = casts
                    .iter()
                    .map(|c| c.caster_npc)
                    .filter(|n| *n != 0)
                    .collect();
                ids.sort_unstable();
                ids.dedup();
                if ids.is_empty() {
                    continue;
                }
                let acc = packs.entry(ids).or_default();
                acc.pulls += 1;
                if let Some(p) = f.pulls.get(pull - 1) {
                    acc.minutes += ((p.end_s - p.start_s) / 60.0).max(0.25);
                }
                for c in casts {
                    acc.names
                        .entry(c.caster_npc)
                        .or_insert_with(|| c.caster.clone());
                    let s = acc
                        .spells
                        .entry(c.name.clone())
                        .or_insert_with(|| SpellNeed {
                            spell: c.name.clone(),
                            tier: c.tier.unwrap_or(1),
                            casts: 0,
                            casts_per_min: 0.0,
                            kicked: 0,
                            stopped: 0,
                            went_off: 0,
                        });
                    s.casts += 1;
                    // Same name under several IDs: the stricter tier wins.
                    s.tier = s.tier.min(c.tier.unwrap_or(1));
                    match &c.outcome {
                        CastOutcome::Kicked { by, .. } => {
                            s.kicked += 1;
                            *acc.kicked_by
                                .entry((by.clone(), c.name.clone()))
                                .or_default() += 1;
                        }
                        CastOutcome::Stopped { .. } => s.stopped += 1,
                        CastOutcome::Completed => {
                            s.went_off += 1;
                            for k in c.ready_kickers.iter().filter(|k| k.in_range == Some(true)) {
                                *acc.in_range
                                    .entry((k.player.clone(), c.name.clone()))
                                    .or_default() += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // The current group: the most recent key's players and their main interrupts.
        let last = keys.last().unwrap();
        let classes = ClassData::shared();
        let group: Vec<String> = last.players.iter().map(|p| p.name.clone()).collect();
        let kickers: Vec<(String, String, f64)> = last
            .players
            .iter()
            .filter_map(|p| {
                let spec = SPECS.iter().find(|s| s.1 == p.class && s.2 == p.spec)?;
                let main = classes.interrupts_for(spec.0).0?;
                Some((p.name.clone(), main.name.clone(), main.cd_s))
            })
            .collect();

        let mut plans: Vec<PackPlan> = packs
            .into_iter()
            .map(|(ids, acc)| plan_pack(ids, acc, &kickers))
            .collect();
        // Most dangerous first: most must-kicks that went off, then all that
        // went off, then busiest.
        plans.sort_by(|a, b| {
            let off1 = |p: &PackPlan| {
                p.spells
                    .iter()
                    .filter(|s| s.tier == 1)
                    .map(|s| s.went_off)
                    .sum::<u32>()
            };
            let off = |p: &PackPlan| p.spells.iter().map(|s| s.went_off).sum::<u32>();
            let rate = |p: &PackPlan| p.spells.iter().map(|s| s.casts_per_min).sum::<f64>();
            off1(b)
                .cmp(&off1(a))
                .then(off(b).cmp(&off(a)))
                .then(rate(b).total_cmp(&rate(a)))
                .then_with(|| a.name.cmp(&b.name))
        });
        plans.truncate(MAX_PACKS);
        out.push(KickPlan {
            dungeon,
            keys: keys.len() as u32,
            group,
            packs: plans,
        });
    }
    out
}

/// "A + B", with variants that share a name counted ("Seductive Sayaad ×2").
fn pack_name<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let mut counts: Vec<(&str, usize)> = vec![];
    for n in names {
        match counts.iter_mut().find(|c| c.0 == n) {
            Some(c) => c.1 += 1,
            None => counts.push((n, 1)),
        }
    }
    counts
        .iter()
        .map(|(n, k)| {
            if *k > 1 {
                format!("{n} ×{k}")
            } else {
                n.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn plan_pack(ids: Vec<u32>, acc: PackAcc, kickers: &[(String, String, f64)]) -> PackPlan {
    let minutes = acc.minutes.max(0.25);
    let mut spells: Vec<SpellNeed> = acc
        .spells
        .into_values()
        .map(|mut s| {
            s.casts_per_min = (100.0 * s.casts as f64 / minutes).round() / 100.0;
            s
        })
        .collect();
    // Must-kicks are assigned first; kick-when-free spells get what's left.
    spells.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(b.casts_per_min.total_cmp(&a.casts_per_min))
            .then_with(|| a.spell.cmp(&b.spell))
    });

    let mut pool: Vec<Kicker> = kickers
        .iter()
        .map(|(n, a, cd)| Kicker {
            name: n.clone(),
            ability: a.clone(),
            capacity: 60.0 / cd,
        })
        .collect();
    let mut assigned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut uncovered = vec![];
    for s in &spells {
        let mut need = s.casts_per_min * MARGIN;
        let score = |k: &Kicker| {
            let kicked = *acc
                .kicked_by
                .get(&(k.name.clone(), s.spell.clone()))
                .unwrap_or(&0);
            let near = *acc
                .in_range
                .get(&(k.name.clone(), s.spell.clone()))
                .unwrap_or(&0);
            (kicked, near)
        };
        let mut order: Vec<usize> = (0..pool.len()).collect();
        order.sort_by(|&a, &b| {
            score(&pool[b])
                .cmp(&score(&pool[a]))
                .then(pool[b].capacity.total_cmp(&pool[a].capacity))
                .then_with(|| pool[a].name.cmp(&pool[b].name))
        });
        for i in order {
            if need <= 0.0 {
                break;
            }
            let k = &mut pool[i];
            if k.capacity <= 0.05 {
                continue;
            }
            let take = k.capacity.min(need);
            k.capacity -= take;
            need -= take;
            assigned
                .entry(k.name.clone())
                .or_default()
                .push(s.spell.clone());
        }
        if need > 0.05 && s.tier == 1 {
            uncovered.push(s.spell.clone());
        }
    }
    let assignments = pool
        .iter()
        .filter_map(|k| {
            assigned.get(&k.name).map(|spells| Assignment {
                player: k.name.clone(),
                ability: k.ability.clone(),
                spells: spells.clone(),
            })
        })
        .collect();
    PackPlan {
        name: pack_name(acc.names.values()),
        npc_ids: ids,
        pulls: acc.pulls,
        spells,
        assignments,
        uncovered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{PullReport, ReadyKicker};

    fn rec(
        npc: u32,
        caster: &str,
        spell: &str,
        at: f64,
        pull: usize,
        outcome: CastOutcome,
    ) -> CastRecord {
        CastRecord {
            spell_id: 1,
            name: spell.into(),
            caster: caster.into(),
            caster_npc: npc,
            start_s: at,
            end_s: at + 2.0,
            pull: Some(pull),
            priority: true,
            tier: Some(1),
            outcome,
            ready_kickers: vec![],
            ready_stoppers: vec![],
        }
    }

    fn kicked(by: &str) -> CastOutcome {
        CastOutcome::Kicked {
            by: by.into(),
            spell: "Kick".into(),
            latency_s: 1.0,
        }
    }

    fn pull(index: usize, start: f64, end: f64) -> PullReport {
        PullReport {
            index,
            start_s: start,
            end_s: end,
            deaths: 0,
            enemies_killed: 0,
            boss: None,
            casts_kicked: 0,
            casts_stopped: 0,
            ..Default::default()
        }
    }

    fn key(
        players: &[(&str, &str, &str)],
        casts: Vec<CastRecord>,
        pulls: Vec<PullReport>,
    ) -> FightReport {
        let json = serde_json::json!({
            "type": "MythicPlus", "zone": "Den of Nalorakk", "map_id": 2825, "level": 13,
            "affixes": [], "success": true, "time_s": 1500.0, "bosses": []
        });
        let mut f = crate::rules::tests::fight_for_plans(serde_json::from_value(json).unwrap());
        f.players = players
            .iter()
            .map(|(n, class, spec)| {
                let mut p = crate::rules::tests::player_for_plans(n);
                p.class = class.to_string();
                p.spec = spec.to_string();
                p
            })
            .collect();
        f.casts = casts;
        f.pulls = pulls;
        f
    }

    #[test]
    fn busy_packs_get_enough_kickers() {
        // Pack A: two Healers (npc 1) casting Healing Breeze 6x a minute; pack
        // B: one Caster (npc 2) with Arc Lightning twice a minute.
        let mut casts = vec![];
        for i in 0..6 {
            let outcome = if i == 0 {
                kicked("Rogue")
            } else {
                CastOutcome::Completed
            };
            let mut c = rec(1, "Healer", "Healing Breeze", 10.0 * i as f64, 1, outcome);
            if i == 1 {
                c.ready_kickers = vec![ReadyKicker {
                    player: "Mage".into(),
                    ability: "Counterspell".into(),
                    distance_yd: Some(20.0),
                    in_range: Some(true),
                    cd_s: 24.0,
                }];
            }
            casts.push(c);
        }
        casts.push(rec(
            2,
            "Caster",
            "Arc Lightning",
            100.0,
            2,
            CastOutcome::Completed,
        ));
        casts.push(rec(
            2,
            "Caster",
            "Arc Lightning",
            130.0,
            2,
            kicked("Warrior"),
        ));
        let players = [
            ("Rogue", "Rogue", "Subtlety"),
            ("Mage", "Mage", "Arcane"),
            ("Warrior", "Warrior", "Protection"),
            ("Priest", "Priest", "Holy"),
        ];
        let f = key(
            &players,
            casts,
            vec![pull(1, 0.0, 60.0), pull(2, 90.0, 150.0)],
        );
        let plans = kick_plans(&[f]);
        assert_eq!(plans.len(), 1);
        let p = &plans[0];
        assert_eq!((p.dungeon.as_str(), p.keys), ("Den of Nalorakk", 1));
        // Healing Breeze: 5 went off, so it's first.
        let a = &p.packs[0];
        assert_eq!((a.name.as_str(), a.pulls), ("Healer", 1));
        assert_eq!(a.spells[0].casts_per_min, 6.0);
        assert_eq!((a.spells[0].kicked, a.spells[0].went_off), (1, 5));
        // 6/min * 1.25 = 7.5 kicks a minute: Rogue (Kick 15s = 4/min, kicked it
        // before) first, then Mage (was in range, 2.5/min), then Warrior (4/min).
        let who: Vec<&str> = a.assignments.iter().map(|x| x.player.as_str()).collect();
        assert_eq!(who, ["Rogue", "Mage", "Warrior"]);
        assert!(a.uncovered.is_empty());
        // The Holy priest has no interrupt and is never assigned.
        assert!(
            p.packs
                .iter()
                .all(|x| x.assignments.iter().all(|y| y.player != "Priest"))
        );
        let b = &p.packs[1];
        assert_eq!(
            b.assignments[0].player, "Warrior",
            "kicked Arc Lightning before"
        );
    }

    #[test]
    fn pack_names_merge_variants() {
        let n = [
            "Sayaad".to_string(),
            "Sayaad".to_string(),
            "Imp".to_string(),
        ];
        assert_eq!(pack_name(n.iter()), "Sayaad ×2 + Imp");
    }

    #[test]
    fn too_busy_for_the_group_is_uncovered() {
        // 20 casts in one minute: one Pummel (4/min) can't cover 25 a minute.
        let casts = (0..20)
            .map(|i| {
                rec(
                    1,
                    "Healer",
                    "Healing Breeze",
                    i as f64 * 3.0,
                    1,
                    CastOutcome::Completed,
                )
            })
            .collect();
        let f = key(
            &[("Warrior", "Warrior", "Arms")],
            casts,
            vec![pull(1, 0.0, 60.0)],
        );
        let p = &kick_plans(&[f])[0].packs[0];
        assert_eq!(p.uncovered, ["Healing Breeze"]);
        assert_eq!(p.assignments.len(), 1);
    }

    #[test]
    fn must_kicks_are_assigned_before_kick_when_free() {
        // One mob type casting Healing Breeze (must kick) twice and Earth Bolt
        // (kick when free) ten times in a minute; one Warrior (4 kicks/min).
        let mut casts = vec![];
        for i in 0..2 {
            casts.push(rec(
                1,
                "Shaman",
                "Healing Breeze",
                i as f64 * 30.0,
                1,
                CastOutcome::Completed,
            ));
        }
        for i in 0..10 {
            let mut c = rec(
                1,
                "Shaman",
                "Earth Bolt",
                i as f64 * 6.0,
                1,
                CastOutcome::Completed,
            );
            (c.priority, c.tier) = (false, Some(2));
            casts.push(c);
        }
        let f = key(
            &[("Warrior", "Warrior", "Arms")],
            casts,
            vec![pull(1, 0.0, 60.0)],
        );
        let p = &kick_plans(&[f])[0].packs[0];
        // Healing Breeze first despite the lower rate; Earth Bolt gets what's
        // left and is never "uncovered".
        let order: Vec<(&str, u8)> = p
            .spells
            .iter()
            .map(|s| (s.spell.as_str(), s.tier))
            .collect();
        assert_eq!(order, [("Healing Breeze", 1), ("Earth Bolt", 2)]);
        assert_eq!(p.assignments[0].spells, ["Healing Breeze", "Earth Bolt"]);
        assert!(p.uncovered.is_empty());
    }

    #[test]
    fn same_pack_across_keys_is_merged_and_raids_ignored() {
        let players = [("Mage", "Mage", "Fire")];
        let k1 = key(
            &players,
            vec![rec(
                1,
                "Healer",
                "Healing Breeze",
                5.0,
                1,
                CastOutcome::Completed,
            )],
            vec![pull(1, 0.0, 60.0)],
        );
        let k2 = key(
            &players,
            vec![rec(1, "Healer", "Healing Breeze", 5.0, 2, kicked("Mage"))],
            vec![pull(1, 0.0, 30.0), pull(2, 40.0, 100.0)],
        );
        let plans = kick_plans(&[k1, k2]);
        let p = &plans[0];
        assert_eq!((p.keys, p.packs.len(), p.packs[0].pulls), (2, 1, 2));
        assert_eq!(
            p.packs[0].spells[0].casts_per_min, 1.0,
            "2 casts over 2 minutes"
        );
    }
}
