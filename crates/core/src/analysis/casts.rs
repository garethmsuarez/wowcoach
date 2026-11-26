//! Enemy cast bars and the group's interrupts, event by event.
//!
//! For every hostile cast bar: how it ended (completed, kicked by whom,
//! stopped by which crowd control, caster killed, or cancelled for no visible
//! reason). For every cast that completed: which players could have kicked it.
//! That means alive, interrupt off cooldown before the bar ended, and how far
//! from the caster, when both positions are fresh. Metrics only; the rules
//! decide what to say about them.

use crate::classes::{Ability, ClassData};
use crate::model::{Event, Kind, Pos, Unit};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Positions older than this aren't used for distances (Warcraft Logs logs
/// positions less often than the game's own combat log).
pub const POS_FRESH_MS: i64 = 3000;
/// An interrupt that only comes off cooldown in the last moment of a cast
/// bar couldn't realistically have been used on it.
pub const REACTION_MS: i64 = 300;
/// Hitbox and position slack added to an interrupt's listed range.
pub const RANGE_SLACK_YD: f32 = 3.0;
/// Two use events of one interrupt this close together are the same kick
/// (the cast and the interrupt are logged separately).
const SAME_USE_MS: i64 = 500;
/// Shortest reuse that's trusted as a real cooldown observation.
const MIN_OBSERVED_CD_MS: i64 = 3000;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CastOutcome {
    Completed,
    Kicked {
        by: String,
        spell: String,
        latency_s: f64,
    },
    Stopped {
        by: String,
        ability: String,
        cc: String,
    },
    CasterDied,
    /// Ended with no completion, kick, stop or death in the log. Not credited.
    Cancelled,
}

/// A player who could have interrupted a cast that completed.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ReadyKicker {
    pub player: String,
    pub ability: String,
    /// Distance to the caster when the cast completed; None if either position was stale.
    pub distance_yd: Option<f32>,
    /// Within the interrupt's range (plus slack), when the distance is known.
    pub in_range: Option<bool>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CastRecord {
    pub spell_id: u32,
    pub name: String,
    pub caster: String,
    /// NPC ID from the caster's GUID (0 when unknown, e.g. Warcraft Logs).
    pub caster_npc: u32,
    pub start_s: f64,
    pub end_s: f64,
    pub pull: Option<usize>,
    pub priority: bool,
    #[serde(flatten)]
    pub outcome: CastOutcome,
    /// Only for completed casts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ready_kickers: Vec<ReadyKicker>,
}

/// What the tracker needs from the fight around it.
pub(super) struct Ctx<'a> {
    pub fight_start: i64,
    pub pull: Option<usize>,
    pub owners: &'a HashMap<String, String>,
    pub specs: &'a HashMap<String, u32>,
    pub priority: &'a dyn Fn(u32) -> bool,
}

struct Bar {
    spell_id: u32,
    name: String,
    caster_guid: String,
    caster: String,
    caster_npc: u32,
    start: i64,
    /// A listed stop landed during the bar: (by, ability, cc).
    stopped: Option<(String, String, String)>,
}

#[derive(Default)]
pub(super) struct Tracker {
    /// Hostile caster guid -> its cast bar in progress.
    bars: HashMap<String, Bar>,
    /// Last known position of every unit.
    pos: HashMap<String, (Pos, i64)>,
    /// (player guid, ability name) -> use timestamps.
    uses: HashMap<(String, String), Vec<i64>>,
    dead: HashSet<String>,
    names: HashMap<String, String>,
    pub records: Vec<CastRecord>,
}

/// NPC ID from a `Creature-0-a-b-c-NPCID-spawn` GUID.
pub fn npc_id(guid: &str) -> u32 {
    guid.split('-')
        .nth(5)
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

impl Tracker {
    /// The group player behind a unit (itself, or a pet's/totem's owner).
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

    fn fresh_pos(&self, guid: &str, at: i64) -> Option<Pos> {
        self.pos
            .get(guid)
            .filter(|(_, t)| at - t <= POS_FRESH_MS && *t <= at + POS_FRESH_MS)
            .map(|(p, _)| *p)
    }

    /// Note a use of an interrupt by a player, merging the cast and the
    /// interrupt event of one kick.
    fn note_use(&mut self, player: &str, ability: &Ability, ts: i64) {
        let v = self
            .uses
            .entry((player.to_string(), ability.name.clone()))
            .or_default();
        if v.last().is_none_or(|l| ts - l > SAME_USE_MS) {
            v.push(ts);
        }
    }

    fn close(&mut self, caster: &str, end: i64, outcome: CastOutcome, ctx: &Ctx) {
        let Some(bar) = self.bars.remove(caster) else {
            return;
        };
        let ready_kickers = if outcome == CastOutcome::Completed {
            self.ready_kickers(&bar, end, ctx)
        } else {
            vec![]
        };
        self.records.push(CastRecord {
            spell_id: bar.spell_id,
            name: bar.name,
            caster: bar.caster,
            caster_npc: bar.caster_npc,
            start_s: secs(bar.start - ctx.fight_start),
            end_s: secs(end - ctx.fight_start),
            pull: ctx.pull,
            priority: (ctx.priority)(bar.spell_id),
            outcome,
            ready_kickers,
        });
    }

    /// Players whose interrupt was available before the bar ended.
    fn ready_kickers(&self, bar: &Bar, end: i64, ctx: &Ctx) -> Vec<ReadyKicker> {
        let classes = ClassData::shared();
        let caster_pos = self.fresh_pos(&bar.caster_guid, end);
        let mut out = vec![];
        let mut players: Vec<(&String, &u32)> = ctx.specs.iter().collect();
        players.sort();
        for (guid, spec) in players {
            if self.dead.contains(guid) || !self.names.contains_key(guid) {
                continue;
            }
            let (main, extra) = classes.interrupts_for(*spec);
            let has = |a: &Ability| self.uses.contains_key(&(guid.clone(), a.name.clone()));
            let options = main.into_iter().chain(extra.into_iter().filter(|a| has(a)));
            // The first ready interrupt is enough to say "could have kicked".
            for a in options {
                let uses = self.uses.get(&(guid.clone(), a.name.clone()));
                let table = (a.cd_s * 1000.0) as i64;
                let observed = uses
                    .into_iter()
                    .flat_map(|u| u.windows(2).map(|w| w[1] - w[0]))
                    .filter(|g| *g >= MIN_OBSERVED_CD_MS)
                    .min()
                    .unwrap_or(table);
                let cd = table.min(observed);
                let last = uses
                    .and_then(|u| u.iter().rev().find(|t| **t <= end))
                    .copied();
                let ready = last.is_none_or(|l| l + cd <= end - REACTION_MS);
                if !ready {
                    continue;
                }
                let distance = match (self.fresh_pos(guid, end), caster_pos) {
                    (Some(p), Some(c)) => p.distance(&c),
                    _ => None,
                };
                let range = a.range_yd.unwrap_or(5.0) + RANGE_SLACK_YD;
                out.push(ReadyKicker {
                    player: self.name(guid),
                    ability: a.name.clone(),
                    distance_yd: distance,
                    in_range: distance.map(|d| d <= range),
                });
                break;
            }
        }
        out
    }

    pub(super) fn feed(&mut self, e: &Event, ctx: &Ctx) {
        // Positions and names.
        let info = match &e.kind {
            Kind::Damage { info, .. }
            | Kind::Heal { info, .. }
            | Kind::CastSuccess { info, .. } => info.as_ref(),
            _ => None,
        };
        if let Some(i) = info
            && let Some(p) = i.pos
        {
            self.pos.insert(i.guid.clone(), (p, e.ts));
        }
        for u in [&e.src, &e.dst].into_iter().flatten() {
            if u.is_group_player() && !self.names.contains_key(&u.guid) {
                self.names.insert(u.guid.clone(), u.name.clone());
            }
        }
        let classes = ClassData::shared();

        match &e.kind {
            Kind::CastStart { spell } => {
                if let Some(c) = e.src.as_ref().filter(|u| u.is_hostile_npc()) {
                    // A new cast replaces one still open: it ended silently.
                    if let Some(prev) = self.bars.get(&c.guid) {
                        let outcome = match &prev.stopped {
                            Some((by, ability, cc)) => CastOutcome::Stopped {
                                by: by.clone(),
                                ability: ability.clone(),
                                cc: cc.clone(),
                            },
                            None => CastOutcome::Cancelled,
                        };
                        self.close(&c.guid, e.ts, outcome, ctx);
                    }
                    self.bars.insert(
                        c.guid.clone(),
                        Bar {
                            spell_id: spell.id,
                            name: spell.name.clone(),
                            caster_guid: c.guid.clone(),
                            caster: c.name.clone(),
                            caster_npc: npc_id(&c.guid),
                            start: e.ts,
                            stopped: None,
                        },
                    );
                }
            }
            Kind::CastSuccess { spell, .. } => {
                let Some(src) = &e.src else { return };
                if src.is_hostile_npc() {
                    if self
                        .bars
                        .get(&src.guid)
                        .is_some_and(|b| b.spell_id == spell.id)
                    {
                        self.close(&src.guid, e.ts, CastOutcome::Completed, ctx);
                    }
                } else if let Some(p) = self.player_of(src, ctx.owners) {
                    self.dead.remove(&p);
                    for a in classes.by_id(spell.id) {
                        if a.kind == crate::classes::AbilityKind::Interrupt {
                            self.note_use(&p, a, e.ts);
                        }
                    }
                }
            }
            Kind::Interrupt { spell, .. } => {
                let (Some(src), Some(dst)) = (&e.src, &e.dst) else {
                    return;
                };
                let by = self.player_of(src, ctx.owners);
                if let Some(p) = &by {
                    for a in classes.by_id(spell.id) {
                        if a.kind == crate::classes::AbilityKind::Interrupt {
                            self.note_use(p, a, e.ts);
                        }
                    }
                }
                if let Some(bar) = self.bars.get(&dst.guid) {
                    let latency = secs(e.ts - bar.start);
                    let who = by
                        .map(|g| self.name(&g))
                        .unwrap_or_else(|| src.name.clone());
                    self.close(
                        &dst.guid,
                        e.ts,
                        CastOutcome::Kicked {
                            by: who,
                            spell: spell.name.clone(),
                            latency_s: latency,
                        },
                        ctx,
                    );
                }
            }
            Kind::AuraApplied {
                spell,
                is_buff: false,
            } => {
                let (Some(src), Some(dst)) = (&e.src, &e.dst) else {
                    return;
                };
                if let Some(stop) = classes.stop_by_aura(spell.id)
                    && let Some(p) = self.player_of(src, ctx.owners)
                {
                    let by = self.name(&p);
                    if let Some(bar) = self.bars.get_mut(&dst.guid)
                        && bar.stopped.is_none()
                    {
                        let cc = stop
                            .cc
                            .map(|c| format!("{c:?}").to_lowercase())
                            .unwrap_or_default();
                        bar.stopped = Some((by, stop.name.clone(), cc));
                    }
                }
            }
            Kind::Death => {
                let Some(d) = &e.dst else { return };
                if d.is_group_player() {
                    self.dead.insert(d.guid.clone());
                } else if let Some(bar) = self.bars.get(&d.guid) {
                    // Stopped first, then killed: the stop is what ended the cast.
                    let outcome = match &bar.stopped {
                        Some((by, ability, cc)) => CastOutcome::Stopped {
                            by: by.clone(),
                            ability: ability.clone(),
                            cc: cc.clone(),
                        },
                        None => CastOutcome::CasterDied,
                    };
                    self.close(&d.guid, e.ts, outcome, ctx);
                }
            }
            _ => {}
        }
    }
}
