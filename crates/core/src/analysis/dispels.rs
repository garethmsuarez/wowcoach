//! Debuffs on the group, and the group's friendly dispels.
//!
//! Every harmful aura a hostile unit puts on a group player is followed to its
//! end: dispelled (by whom, how fast), expired, or the player died with it up.
//! For priority debuffs that weren't dispelled, it lists who could have:
//! alive, with a dispel of a matching type off cooldown while the debuff was
//! up, and in range when positions are fresh. Metrics only; the rules decide
//! what to say about them.

use crate::classes::{Ability, AbilityKind, ClassData, DispelType, Target};
use crate::model::{Event, Kind, Pos, Unit};
use crate::specs::{Role, spec_info};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A debuff's removal and the `SPELL_DISPEL` that caused it are logged
/// separately (the removal usually first), at most this far apart.
pub const DISPEL_PAIR_MS: i64 = 300;
/// A dispel must be off cooldown this long before the debuff ends to count as
/// ready (nobody reacts in the last frame).
pub const REACTION_MS: i64 = 500;
/// Friendly dispels reach this far (all listed ones are 40 yd), plus slack.
pub const DISPEL_RANGE_YD: f32 = 40.0;
const RANGE_SLACK_YD: f32 = 3.0;
/// Positions older than this aren't used for distances.
const POS_FRESH_MS: i64 = 3000;
/// Dispels with a longer cooldown (Mass Dispel, Revival) only count as ready
/// once used in this fight; so do any non-healer's dispels (often talents).
const SHORT_DISPEL_CD_S: f64 = 15.0;
const MIN_OBSERVED_CD_MS: i64 = 3000;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DebuffOutcome {
    Dispelled {
        by: String,
        ability: String,
        latency_s: f64,
    },
    Expired,
    /// The player died with it up.
    Died,
    /// Still up when the fight ended.
    Lasted,
}

/// A player who could have dispelled a priority debuff that wasn't.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ReadyDispeller {
    pub player: String,
    pub ability: String,
    /// Distance to the debuffed player when the dispel came off cooldown
    /// during the debuff; None if either position was stale.
    pub distance_yd: Option<f32>,
    pub in_range: Option<bool>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DebuffRecord {
    pub spell_id: u32,
    pub name: String,
    pub player: String,
    /// The hostile unit that applied it.
    pub source: String,
    pub applied_s: f64,
    pub ended_s: f64,
    pub pull: Option<usize>,
    /// Tagged `dispel_priority`.
    pub priority: bool,
    /// From the pack's `dispel`, else what removed it in this fight; empty
    /// when unknown (then nobody is named as a ready dispeller).
    pub types: Vec<DispelType>,
    #[serde(flatten)]
    pub outcome: DebuffOutcome,
    /// Only for priority debuffs that weren't dispelled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ready_dispellers: Vec<ReadyDispeller>,
}

/// What the tracker needs from the fight around it.
pub(super) struct Ctx<'a> {
    pub owners: &'a HashMap<String, String>,
}

/// Settings and lookups `finish` needs.
pub(super) struct Fin<'a> {
    pub fight_start: i64,
    pub end_ts: i64,
    pub specs: &'a HashMap<String, u32>,
    pub priority: &'a dyn Fn(u32) -> bool,
    pub pack_types: &'a dyn Fn(u32) -> Vec<DispelType>,
    /// Pull windows (start, end): records are numbered by these.
    pub pulls: Vec<(i64, i64)>,
}

impl Fin<'_> {
    fn pull_at(&self, ts: i64) -> Option<usize> {
        self.pulls
            .iter()
            .position(|(s, e)| *s <= ts && ts <= *e)
            .map(|i| i + 1)
    }
}

struct Open {
    spell_id: u32,
    name: String,
    player: String,
    source: String,
    applied: i64,
    /// (ts, how) once closed.
    end: Option<(i64, End)>,
}

#[derive(Clone)]
enum End {
    Dispelled { by: String, ability: usize },
    Removed,
    Died,
}

#[derive(Default)]
pub(super) struct Tracker {
    names: HashMap<String, String>,
    /// Every debuff, in order of application.
    debuffs: Vec<Open>,
    /// (player guid, debuff id) -> index in `debuffs` while it's up.
    open: HashMap<(String, u32), usize>,
    /// (player guid, debuff id) -> index of the last one closed by a removal,
    /// so a dispel logged just after can claim it.
    removed: HashMap<(String, u32), usize>,
    /// (player guid, dispel ability index) -> dispel timestamps.
    uses: BTreeMap<(String, usize), Vec<i64>>,
    /// Debuff id -> dispel abilities that removed it in this fight.
    removed_by: HashMap<u32, BTreeSet<usize>>,
    dead: HashMap<String, Vec<(i64, Option<i64>)>>,
    pos: HashMap<String, Vec<(i64, Pos)>>,
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

fn ability(i: usize) -> &'static Ability {
    &ClassData::shared().abilities[i]
}

/// Friendly dispels in the class table, by spell ID.
fn friendly_dispels(id: u32) -> Option<usize> {
    ClassData::shared().abilities.iter().position(|a| {
        a.kind == AbilityKind::Dispel && a.target == Some(Target::Friendly) && a.ids.contains(&id)
    })
}

impl Tracker {
    fn player_of(&self, u: &Unit, owners: &HashMap<String, String>) -> Option<String> {
        if u.is_player() {
            return Some(u.guid.clone());
        }
        if u.is_hostile() {
            return None;
        }
        let mut g = owners.get(&u.guid)?;
        for _ in 0..3 {
            if g.starts_with("Player-") {
                return Some(g.clone());
            }
            g = owners.get(g)?;
        }
        None
    }

    fn name(&self, guid: &str) -> String {
        self.names
            .get(guid)
            .cloned()
            .unwrap_or_else(|| guid.to_string())
    }

    fn close(&mut self, i: usize, ts: i64, how: End) {
        let d = &mut self.debuffs[i];
        if d.end.is_none() {
            d.end = Some((ts, how));
        }
    }

    fn note_dispel(&mut self, by: &str, ab: usize, debuff: u32, ts: i64) {
        let v = self.uses.entry((by.to_string(), ab)).or_default();
        if v.last().is_none_or(|l| ts - l > DISPEL_PAIR_MS) {
            v.push(ts);
        }
        self.removed_by.entry(debuff).or_default().insert(ab);
    }

    pub(super) fn feed(&mut self, e: &Event, ctx: &Ctx) {
        let ts = e.ts;
        for u in [&e.src, &e.dst].into_iter().flatten() {
            if u.is_group_player() && !self.names.contains_key(&u.guid) {
                self.names.insert(u.guid.clone(), u.name.clone());
            }
        }
        let info = match &e.kind {
            Kind::Damage { info, .. }
            | Kind::Heal { info, .. }
            | Kind::CastSuccess { info, .. } => info.as_ref(),
            _ => None,
        };
        if let Some(i) = info
            && let Some(p) = i.pos
            && self.names.contains_key(&i.guid)
        {
            self.pos.entry(i.guid.clone()).or_default().push((ts, p));
        }
        match &e.kind {
            Kind::AuraApplied {
                spell,
                is_buff: false,
            } => {
                let (Some(s), Some(d)) = (&e.src, &e.dst) else {
                    return;
                };
                if !s.is_hostile_npc() || !d.is_group_player() {
                    return;
                }
                let key = (d.guid.clone(), spell.id);
                if self.open.contains_key(&key) {
                    return; // a refresh or another stack
                }
                self.open.insert(key, self.debuffs.len());
                self.debuffs.push(Open {
                    spell_id: spell.id,
                    name: spell.name.clone(),
                    player: d.guid.clone(),
                    source: s.name.clone(),
                    applied: ts,
                    end: None,
                });
            }
            Kind::AuraRemoved {
                spell,
                is_buff: false,
            } => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_group_player()) else {
                    return;
                };
                let key = (d.guid.clone(), spell.id);
                // Already closed by a dispel logged first: nothing to do.
                let Some(i) = self.open.remove(&key) else {
                    return;
                };
                self.close(i, ts, End::Removed);
                self.removed.insert(key, i);
            }
            Kind::Dispel { spell, extra } => {
                let (Some(s), Some(d)) = (&e.src, &e.dst) else {
                    return;
                };
                if !d.is_group_player() {
                    return;
                }
                let (Some(by), Some(ab)) =
                    (self.player_of(s, ctx.owners), friendly_dispels(spell.id))
                else {
                    return;
                };
                self.note_dispel(&by, ab, extra.id, ts);
                let key = (d.guid.clone(), extra.id);
                // Usually the removal came first: claim it if it was just now.
                if let Some(&i) = self.removed.get(&key)
                    && let Some((t, End::Removed)) = self.debuffs[i].end.clone()
                    && ts - t <= DISPEL_PAIR_MS
                {
                    self.removed.remove(&key);
                    self.debuffs[i].end = Some((t, End::Dispelled { by, ability: ab }));
                } else if let Some(i) = self.open.remove(&key) {
                    // The dispel came first (or no removal is logged at all).
                    self.close(i, ts, End::Dispelled { by, ability: ab });
                }
            }
            Kind::CastSuccess { .. } => {
                if let Some(src) = &e.src
                    && let Some(p) = self.player_of(src, ctx.owners)
                    && let Some(w) = self.dead.get_mut(&p).and_then(|v| v.last_mut())
                    && w.1.is_none()
                {
                    w.1 = Some(ts);
                }
            }
            Kind::Death => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_group_player()) else {
                    return;
                };
                let v = self.dead.entry(d.guid.clone()).or_default();
                if v.last().is_none_or(|w| w.1.is_some()) {
                    v.push((ts, None));
                }
                let mine: Vec<(String, u32)> = self
                    .open
                    .keys()
                    .filter(|(g, _)| *g == d.guid)
                    .cloned()
                    .collect();
                for k in mine {
                    if let Some(i) = self.open.remove(&k) {
                        self.close(i, ts, End::Died);
                    }
                }
            }
            _ => {}
        }
    }

    fn alive(&self, guid: &str, t: i64) -> bool {
        !self
            .dead
            .get(guid)
            .into_iter()
            .flatten()
            .any(|(s, e)| *s <= t && e.is_none_or(|e| t < e))
    }

    fn pos_at(&self, guid: &str, t: i64) -> Option<Pos> {
        self.pos
            .get(guid)?
            .iter()
            .rev()
            .find(|(pt, _)| *pt <= t + POS_FRESH_MS)
            .filter(|(pt, _)| (t - pt).abs() <= POS_FRESH_MS)
            .map(|(_, p)| *p)
    }

    fn cd_ms(&self, guid: &str, ab: usize) -> i64 {
        let table = (ability(ab).cd_s * 1000.0) as i64;
        let floor = MIN_OBSERVED_CD_MS.max(table / 2);
        self.uses
            .get(&(guid.to_string(), ab))
            .into_iter()
            .flat_map(|u| u.windows(2).map(|w| w[1] - w[0]))
            .filter(|g| *g >= floor)
            .min()
            .unwrap_or(table)
            .min(table)
    }

    /// Types of a debuff: the pack's, else what all its dispels in this fight share.
    fn types(&self, id: u32, pack: &[DispelType]) -> Vec<DispelType> {
        if !pack.is_empty() {
            return pack.to_vec();
        }
        let mut common: Option<BTreeSet<DispelType>> = None;
        for ab in self.removed_by.get(&id).into_iter().flatten() {
            let t: BTreeSet<DispelType> = ability(*ab).types.iter().copied().collect();
            common = Some(match common {
                None => t,
                Some(c) => c.intersection(&t).copied().collect(),
            });
        }
        common.unwrap_or_default().into_iter().collect()
    }

    /// Players who could have dispelled `d` (of `types`) between `from` and `to`.
    fn ready(
        &self,
        d: &Open,
        types: &[DispelType],
        from: i64,
        to: i64,
        specs: &HashMap<String, u32>,
    ) -> Vec<ReadyDispeller> {
        let classes = ClassData::shared();
        let mut players: Vec<(&String, &u32)> = specs
            .iter()
            .filter(|(g, _)| self.names.contains_key(*g))
            .collect();
        players.sort_by_key(|(g, _)| self.name(g));
        let mut out = vec![];
        for (guid, spec) in players {
            let healer = spec_info(*spec).is_some_and(|(_, _, r)| r == Role::Healer);
            for a in classes.for_spec(*spec, AbilityKind::Dispel) {
                // When the type is ambiguous (magic or disease), only a dispel
                // that covers every candidate surely could have.
                if a.target != Some(Target::Friendly) || !types.iter().all(|t| a.types.contains(t))
                {
                    continue;
                }
                let Some(ab) = classes.abilities.iter().position(|x| std::ptr::eq(x, a)) else {
                    continue;
                };
                let uses = self.uses.get(&(guid.clone(), ab));
                let used_before = uses.is_some_and(|u| u.iter().any(|t| *t < to));
                if !(healer && a.cd_s <= SHORT_DISPEL_CD_S) && !used_before {
                    continue;
                }
                // Dispelling something else meanwhile is busy, not idle.
                if uses.is_some_and(|u| u.iter().any(|t| *t > from && *t <= to)) {
                    continue;
                }
                // When it was off cooldown during the debuff.
                let last = uses.and_then(|u| u.iter().rev().find(|t| **t <= from).copied());
                let at = last.map_or(from, |l| (l + self.cd_ms(guid, ab)).max(from));
                if at > to - REACTION_MS || !self.alive(guid, at) {
                    continue;
                }
                let dist = match (self.pos_at(guid, at), self.pos_at(&d.player, at)) {
                    (Some(a), Some(b)) => a.distance(&b),
                    _ => None,
                };
                out.push(ReadyDispeller {
                    player: self.name(guid),
                    ability: a.name.clone(),
                    distance_yd: dist,
                    in_range: dist.map(|x| x <= DISPEL_RANGE_YD + RANGE_SLACK_YD),
                });
                break; // one ready dispel per player is enough
            }
        }
        out
    }

    pub(super) fn finish(&self, fin: &Fin) -> Vec<DebuffRecord> {
        let rel = |t: i64| secs(t - fin.fight_start);
        self.debuffs
            .iter()
            .map(|d| {
                let (end, outcome) = match &d.end {
                    Some((t, End::Dispelled { by, ability: ab })) => (
                        *t,
                        DebuffOutcome::Dispelled {
                            by: self.name(by),
                            ability: ability(*ab).name.clone(),
                            latency_s: secs(t - d.applied),
                        },
                    ),
                    Some((t, End::Removed)) => (*t, DebuffOutcome::Expired),
                    Some((t, End::Died)) => (*t, DebuffOutcome::Died),
                    None => (fin.end_ts, DebuffOutcome::Lasted),
                };
                let priority = (fin.priority)(d.spell_id);
                let types = self.types(d.spell_id, &(fin.pack_types)(d.spell_id));
                let dispelled = matches!(outcome, DebuffOutcome::Dispelled { .. });
                let ready_dispellers = if priority && !dispelled && !types.is_empty() {
                    self.ready(d, &types, d.applied, end, fin.specs)
                } else {
                    vec![]
                };
                DebuffRecord {
                    spell_id: d.spell_id,
                    name: d.name.clone(),
                    player: self.name(&d.player),
                    source: d.source.clone(),
                    applied_s: rel(d.applied),
                    ended_s: rel(end),
                    pull: fin.pull_at(d.applied),
                    priority,
                    types,
                    outcome,
                    ready_dispellers,
                }
            })
            .collect()
    }
}
