//! Top public runs as a benchmark for a key's route and pacing (M5).
//!
//! A `RefRun` is one public run reduced to game data: its pull windows, its
//! boss pulls and its enemy kills (NPC ID and time). That is what
//! `analyze --compare-top N` caches. `summarize` turns several of them, plus the
//! pack's forces table, into `TopRuns`: the median and range of each boss
//! kill time, % forces at each boss, and each segment's time, combat and
//! downtime, lined up with your key's bosses.
//!
//! Each run also carries the damage its non-tank players took from each enemy
//! spell (`RefDamage`), so `summarize` can say how much of a spell your group
//! takes against the top runs, per player and per 10 minutes of combat.
//!
//! Forces are computed here, not cached, so a cached run stays valid when the
//! forces table changes. Public runs only log deaths, so units scripted away
//! at 1 HP (Kings' Rest's Shadow of Zul) are missing from their forces.

use crate::analysis::{BossMarker, Segment};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One public run, reduced to game data. Times are milliseconds from the
/// run's start.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RefRun {
    pub code: String,
    pub fight_id: i64,
    pub map_id: u32,
    pub level: u32,
    pub affixes: Vec<u32>,
    /// Key time, death penalties included.
    pub time_ms: i64,
    /// Upgrades (0 = depleted), when Warcraft Logs reports them.
    pub bonus: Option<u8>,
    pub pulls: Vec<RefPull>,
    /// (ms from start, NPC ID) for each enemy death.
    pub kills: Vec<(i64, u32)>,
    /// Players who weren't tanks; 0 when the run has no damage data.
    #[serde(default)]
    pub non_tanks: u32,
    /// Damage the non-tank players took from enemies (and the environment),
    /// per spell, by spell ID.
    #[serde(default)]
    pub damage: Vec<RefDamage>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RefDamage {
    pub spell_id: u32,
    pub hits: u32,
    /// Damage after mitigation, absorbs included (as `EnemySpellReport.total`).
    pub amount: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RefPull {
    pub start_ms: i64,
    pub end_ms: i64,
    /// The boss encounter (as logged by `ENCOUNTER_START`); 0 for trash.
    pub encounter_id: u32,
    pub kill: bool,
}

/// Median and range over the runs.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Stat {
    pub median: f64,
    pub min: f64,
    pub max: f64,
}

impl Stat {
    pub fn of(mut v: Vec<f64>) -> Option<Stat> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        };
        Some(Stat {
            median,
            min: v[0],
            max: v[n - 1],
        })
    }
}

/// The benchmark for one key: top public runs of the same dungeon and level.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct TopRuns {
    /// "code:fight" of each run used.
    pub runs: Vec<String>,
    pub level: u32,
    pub time_s: Stat,
    /// Forces at the end of the runs (with a forces table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forces_pct: Option<Stat>,
    /// Your killed bosses, in your order, as the runs killed them.
    pub bosses: Vec<RefBoss>,
    /// Your segments, as the runs played them.
    pub segments: Vec<RefSegment>,
    /// How many of the runs have damage data.
    pub damage_runs: usize,
    /// For each spell you took damage from (merged by name), what the runs'
    /// non-tank players took, per player and per 10 minutes of combat. A run
    /// that never took it counts as 0.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub damage: Vec<RefSpell>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RefSpell {
    pub name: String,
    pub damage: Stat,
    pub hits: Stat,
}

/// Rates are per this much combat: 10 minutes.
pub const RATE_MS: f64 = 600_000.0;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RefBoss {
    pub name: String,
    pub encounter_id: u32,
    /// When the boss died, seconds from the start.
    pub kill_s: Stat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forces_pct: Option<Stat>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RefSegment {
    pub index: usize,
    pub boss: Option<String>,
    pub duration_s: Stat,
    pub combat_s: Stat,
    pub downtime_s: Stat,
    /// Forces gained in the segment, % of the total (with a forces table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forces_pct: Option<Stat>,
}

/// Where a run's segments end: (ms, encounter ID) at each boss kill, then the
/// end of the run when pulls follow the last kill.
fn cuts(r: &RefRun) -> Vec<(i64, u32)> {
    let mut v: Vec<(i64, u32)> = vec![];
    for p in r.pulls.iter().filter(|p| p.encounter_id != 0 && p.kill) {
        match v.iter_mut().find(|c| c.1 == p.encounter_id) {
            Some(c) => c.0 = c.0.max(p.end_ms),
            None => v.push((p.end_ms, p.encounter_id)),
        }
    }
    v.sort();
    v
}

/// Total time covered by pull windows within [a, b].
fn combat_ms(r: &RefRun, a: i64, b: i64) -> i64 {
    let mut w: Vec<(i64, i64)> = r
        .pulls
        .iter()
        .map(|p| (p.start_ms.max(a), p.end_ms.min(b)))
        .filter(|(s, e)| s < e)
        .collect();
    w.sort();
    let mut total = 0;
    let mut cur: Option<(i64, i64)> = None;
    for (s, e) in w {
        match &mut cur {
            Some(c) if s <= c.1 => c.1 = c.1.max(e),
            _ => {
                if let Some(c) = cur {
                    total += c.1 - c.0;
                }
                cur = Some((s, e));
            }
        }
    }
    total + cur.map_or(0, |c| c.1 - c.0)
}

/// Line the runs up with your key. `forces` is (total, forces per NPC ID).
/// `spells` names the spells you took damage from (spell ID → name); the
/// runs' damage is compared for those only. Runs missing one of your killed
/// bosses are left out of that boss and its segment. None without runs.
pub fn summarize(
    runs: &[RefRun],
    level: u32,
    bosses: &[BossMarker],
    segments: &[Segment],
    forces: Option<(u32, &HashMap<u32, u32>)>,
    spells: &HashMap<u32, String>,
) -> Option<TopRuns> {
    if runs.is_empty() {
        return None;
    }
    let pct = |r: &RefRun, until: i64| -> Option<f64> {
        let (total, table) = forces?;
        let n: u32 = r
            .kills
            .iter()
            .filter(|(t, _)| *t <= until)
            .map(|(_, id)| table.get(id).copied().unwrap_or(0))
            .sum();
        Some(100.0 * n as f64 / total.max(1) as f64)
    };
    let secs = |ms: i64| ms as f64 / 1000.0;
    let killed: Vec<&BossMarker> = bosses.iter().filter(|b| b.success).collect();
    let boss_at = |r: &RefRun, enc: u32| cuts(r).into_iter().find(|c| c.1 == enc).map(|c| c.0);
    let opt_stat = |v: Vec<Option<f64>>| -> Option<Stat> {
        if v.iter().any(Option::is_none) {
            None
        } else {
            Stat::of(v.into_iter().flatten().collect())
        }
    };

    let ref_bosses = killed
        .iter()
        .filter_map(|b| {
            let at: Vec<(&RefRun, i64)> = runs
                .iter()
                .filter_map(|r| boss_at(r, b.encounter_id).map(|t| (r, t)))
                .collect();
            Some(RefBoss {
                name: b.name.clone(),
                encounter_id: b.encounter_id,
                kill_s: Stat::of(at.iter().map(|(_, t)| secs(*t)).collect())?,
                forces_pct: opt_stat(at.iter().map(|(r, t)| pct(r, *t)).collect()),
            })
        })
        .collect();

    // A run's segment for each of yours: from the previous boss kill (or the
    // start) to this boss's kill (or, for trailing trash, the run's end).
    let ref_segments = segments
        .iter()
        .filter_map(|s| {
            let enc = |name: &Option<String>| {
                killed
                    .iter()
                    .find(|b| Some(&b.name) == name.as_ref())
                    .map(|b| b.encounter_id)
            };
            let prev = if s.index > 1 {
                segments.get(s.index - 2).and_then(|p| enc(&p.boss))
            } else {
                None
            };
            let spans: Vec<(&RefRun, i64, i64)> = runs
                .iter()
                .filter_map(|r| {
                    let a = match prev {
                        Some(e) => boss_at(r, e)?,
                        None if s.index == 1 => 0,
                        None => return None,
                    };
                    let b = match enc(&s.boss) {
                        Some(e) => boss_at(r, e)?,
                        None => r.pulls.iter().map(|p| p.end_ms).max()?.max(a),
                    };
                    (b > a).then_some((r, a, b))
                })
                .collect();
            Some(RefSegment {
                index: s.index,
                boss: s.boss.clone(),
                duration_s: Stat::of(spans.iter().map(|(_, a, b)| secs(b - a)).collect())?,
                combat_s: Stat::of(
                    spans
                        .iter()
                        .map(|(r, a, b)| secs(combat_ms(r, *a, *b)))
                        .collect(),
                )?,
                downtime_s: Stat::of(
                    spans
                        .iter()
                        .map(|(r, a, b)| secs(b - a - combat_ms(r, *a, *b)))
                        .collect(),
                )?,
                forces_pct: opt_stat(
                    spans
                        .iter()
                        .map(|(r, a, b)| Some(pct(r, *b)? - pct(r, *a - 1)?))
                        .collect(),
                ),
            })
        })
        .collect();

    Some(TopRuns {
        runs: runs
            .iter()
            .map(|r| format!("{}:{}", r.code, r.fight_id))
            .collect(),
        level,
        time_s: Stat::of(runs.iter().map(|r| secs(r.time_ms)).collect())?,
        forces_pct: opt_stat(runs.iter().map(|r| pct(r, i64::MAX)).collect()),
        bosses: ref_bosses,
        segments: ref_segments,
        damage_runs: runs.iter().filter(|r| r.non_tanks > 0).count(),
        damage: damage(runs, spells),
    })
}

/// Per spell name, each damage run's non-tank damage and hits per player per
/// 10 minutes of combat, as a `Stat` over the runs. Names in order.
fn damage(runs: &[RefRun], spells: &HashMap<u32, String>) -> Vec<RefSpell> {
    let runs: Vec<&RefRun> = runs.iter().filter(|r| r.non_tanks > 0).collect();
    let mut names: Vec<&String> = spells.values().collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|name| {
            let (mut dmg, mut hits) = (vec![], vec![]);
            for r in &runs {
                let per = r.non_tanks as f64 * combat_ms(r, 0, i64::MAX).max(1) as f64 / RATE_MS;
                let (a, h) = r
                    .damage
                    .iter()
                    .filter(|d| spells.get(&d.spell_id) == Some(name))
                    .fold((0u64, 0u32), |(a, h), d| (a + d.amount, h + d.hits));
                dmg.push(a as f64 / per);
                hits.push(h as f64 / per);
            }
            Some(RefSpell {
                name: name.clone(),
                damage: Stat::of(dmg)?,
                hits: Stat::of(hits)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
