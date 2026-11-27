//! Kills through a key, for enemy forces and route analysis.
//!
//! A kill is a hostile NPC's first `UNIT_DIED`, or a unit last seen at
//! `forces::DEFEATED_HP_FRAC` health or less that never dies (some are
//! scripted away at 1 HP, like Kings' Rest's Shadow of Zul). Units summoned
//! by enemies are marked, so counts of what was pulled can leave them out.
//! Metrics only.

use super::casts::npc_id;
use crate::forces::DEFEATED_HP_FRAC;
use crate::model::{Event, Kind};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Kill {
    pub ts: i64,
    pub guid: String,
    pub npc: u32,
    pub name: String,
    /// Summoned by an enemy (`SPELL_SUMMON`, or logged with an owner).
    pub summoned: bool,
}

/// Advanced logging writes this for "no owner".
const NO_OWNER: &str = "0000000000000000";

#[derive(Default)]
pub(super) struct Tracker {
    dead: HashSet<String>,
    kills: Vec<Kill>,
    /// Hostile GUID -> (ts, health fraction, NPC ID, name) at its last hit,
    /// while it hasn't died.
    last_hp: HashMap<String, (i64, f64, u32, String)>,
    summoned: HashSet<String>,
}

impl Tracker {
    pub(super) fn feed(&mut self, e: &Event) {
        match &e.kind {
            Kind::Summon { .. } => {
                if let (Some(s), Some(d)) = (&e.src, &e.dst)
                    && s.is_hostile()
                {
                    self.summoned.insert(d.guid.clone());
                }
            }
            Kind::Damage { info: Some(i), .. } => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_hostile_npc()) else {
                    return;
                };
                if i.guid != d.guid || i.max_hp == 0 || self.dead.contains(&d.guid) {
                    return;
                }
                if !i.owner_guid.is_empty() && i.owner_guid != NO_OWNER {
                    self.summoned.insert(d.guid.clone());
                }
                let npc = npc_id(&d.guid);
                if npc != 0 {
                    self.last_hp.insert(
                        d.guid.clone(),
                        (e.ts, i.hp as f64 / i.max_hp as f64, npc, d.name.clone()),
                    );
                }
            }
            Kind::Death => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_hostile_npc()) else {
                    return;
                };
                let npc = npc_id(&d.guid);
                if npc == 0 || !self.dead.insert(d.guid.clone()) {
                    return;
                }
                self.last_hp.remove(&d.guid);
                self.kills.push(Kill {
                    ts: e.ts,
                    guid: d.guid.clone(),
                    npc,
                    name: d.name.clone(),
                    summoned: self.summoned.contains(&d.guid),
                });
            }
            _ => {}
        }
    }

    /// Every kill in time order, including units defeated at 1 HP (timed at
    /// their last hit).
    pub(super) fn finish(&self) -> Vec<Kill> {
        let mut out = self.kills.clone();
        for (g, (ts, frac, npc, name)) in &self.last_hp {
            if *frac <= DEFEATED_HP_FRAC {
                out.push(Kill {
                    ts: *ts,
                    guid: g.clone(),
                    npc: *npc,
                    name: name.clone(),
                    summoned: self.summoned.contains(g),
                });
            }
        }
        out.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.guid.cmp(&b.guid)));
        out
    }
}
