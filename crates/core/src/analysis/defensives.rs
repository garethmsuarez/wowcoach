//! Defensives, damage spikes, tank busters, enemy summons and lust.
//!
//! While events stream in, the tracker only records history: which player
//! used which class-table defensive when, which defensive and mitigation
//! buffs were up on whom, group damage taken, tank-buster hits, units enemies
//! summoned, and Sated debuffs. `finish` then answers questions about moments
//! in the fight: what was active on a player, which defensives were ready.
//! Metrics only; the rules decide what to say about them.

use crate::classes::{Ability, AbilityKind, ClassData, Scope};
use crate::model::{Event, Kind, Spell, Unit};
use crate::specs::{Role, spec_info};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use super::casts::npc_id;

/// Two use events of one defensive this close together are the same use (the
/// raw log often writes the buff just before the cast).
const SAME_USE_MS: i64 = 500;
/// Shortest reuse that's trusted as a real cooldown observation.
const MIN_OBSERVED_CD_MS: i64 = 3000;
/// A death counts toward a spike when it happens during it or this soon after.
pub const SPIKE_DEATH_AFTER_MS: i64 = 5000;
/// A group defensive used this long before a spike started still answers it.
pub const SPIKE_LEAD_MS: i64 = 2000;
/// A defensive has to be off cooldown this long before a death to count as
/// ready (nobody presses a button in the last frame).
pub const DEATH_REACTION_MS: i64 = 1000;
/// Spells listed per spike.
const SPIKE_TOP: usize = 3;
/// Sated lasts this long. A lust cast with no Sated debuff logged within
/// `SATED_MATCH_MS` of it is assumed to have applied one.
pub const SATED_MS: i64 = 600_000;
const SATED_MATCH_MS: i64 = 3000;
/// Classes that can cast lust themselves (hunters through their pet).
const LUST_CLASSES: [&str; 4] = ["Shaman", "Mage", "Evoker", "Hunter"];

/// A use of a class-table defensive (not mitigation, which is rotational).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DefensiveUse {
    pub player: String,
    pub ability: String,
    pub scope: String,
    pub at_s: f64,
    pub pull: Option<usize>,
}

/// A defensive that was off cooldown at some moment, and that its player had
/// already used in this fight (most defensives are talents or choice nodes).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ReadyDefensive {
    pub player: String,
    pub ability: String,
    pub last_used_s: f64,
}

/// A group defensive used during (or just before) a spike.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct GroupDefensive {
    pub player: String,
    pub ability: String,
    pub at_s: f64,
}

/// One of a spike's top damage sources.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SpikeSpell {
    pub spell_id: u32,
    pub name: String,
    pub source: String,
    /// NPC ID of the source (0 when unknown).
    pub source_npc: u32,
    pub damage: u64,
    /// The source was summoned by an enemy (a totem): who summoned it, and
    /// how long it had been alive when the spike started (the oldest one, if
    /// several summons of that kind hit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summoned_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summon_age_s: Option<f64>,
}

/// A stretch where the group lost at least `spike_frac` of its summed max HP
/// within `spike_window_ms` (damage after absorbs). Overlapping windows are merged.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Spike {
    pub pull: Option<usize>,
    pub start_s: f64,
    pub end_s: f64,
    /// Group damage taken from start to end.
    pub damage: u64,
    /// Highest damage in one window, as a share of the group's max HP.
    pub peak_frac: f64,
    pub top: Vec<SpikeSpell>,
    /// Players who died during the spike or within `SPIKE_DEATH_AFTER_MS`.
    pub deaths: Vec<String>,
    pub group_used: Vec<GroupDefensive>,
    /// Group defensives off cooldown when the spike started, that weren't
    /// then used in it.
    pub group_ready: Vec<ReadyDefensive>,
}

/// A hit from a `tank_buster` spell on a tank.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TankHit {
    pub pull: Option<usize>,
    pub at_s: f64,
    pub tank: String,
    pub spell_id: u32,
    pub name: String,
    pub school: u32,
    pub amount: u64,
    /// Mitigation buffs up that work against this hit's school.
    pub mitigation: Vec<String>,
    /// Defensives up on the tank (their own, externals, group) that work
    /// against it.
    pub defensives: Vec<String>,
}

/// A unit an enemy summoned (a totem, an add).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EnemySummon {
    pub pull: Option<usize>,
    pub name: String,
    pub npc: u32,
    pub summoner: String,
    pub summoner_npc: u32,
    pub spell: String,
    pub start_s: f64,
    /// When it died, or the fight ended.
    pub end_s: f64,
    pub killed: bool,
    pub damage_to_group: u64,
    pub healing_to_enemies: u64,
}

/// What the tracker needs from the fight around it.
pub(super) struct Ctx<'a> {
    pub owners: &'a HashMap<String, String>,
    pub specs: &'a HashMap<String, u32>,
    pub tank_buster: &'a dyn Fn(u32) -> bool,
    pub sated: &'a [u32],
    /// Damage that only shows up later (Stagger): not counted twice.
    pub deferred: &'a [u32],
}

/// Defensives and mitigation in the class table, by cast and by aura ID.
struct Index {
    by_cast: HashMap<u32, Vec<usize>>,
    by_aura: HashMap<u32, usize>,
}

fn index() -> &'static Index {
    static I: OnceLock<Index> = OnceLock::new();
    I.get_or_init(|| {
        let mut ix = Index {
            by_cast: HashMap::new(),
            by_aura: HashMap::new(),
        };
        for (i, a) in ClassData::shared().abilities.iter().enumerate() {
            if !matches!(a.kind, AbilityKind::Defensive | AbilityKind::Mitigation) {
                continue;
            }
            for id in &a.ids {
                ix.by_cast.entry(*id).or_default().push(i);
            }
            if !a.instant {
                for id in a.auras() {
                    ix.by_aura.entry(*id).or_insert(i);
                }
            }
        }
        ix
    })
}

fn ability(i: usize) -> &'static Ability {
    &ClassData::shared().abilities[i]
}

/// A defensive or mitigation buff that was up.
struct Iv {
    /// On whom; None = the whole group (a group defensive without a logged buff).
    target: Option<String>,
    ability: usize,
    start: i64,
    end: Option<i64>,
}

struct Hit {
    ts: i64,
    amount: u64,
    spell_id: u32,
    src: String,
}

struct Summon {
    guid: String,
    name: String,
    summoner: String,
    summoner_npc: u32,
    spell: String,
    start: i64,
    end: Option<i64>,
    damage: u64,
    healing: u64,
}

#[derive(Default)]
pub(super) struct Tracker {
    names: HashMap<String, String>,
    max_hp: HashMap<String, u64>,
    /// (player guid, ability index) -> use timestamps.
    uses: BTreeMap<(String, usize), Vec<i64>>,
    /// Defensive uses in order, for the output: (ts, player guid, ability).
    use_log: Vec<(i64, String, usize)>,
    ivs: Vec<Iv>,
    /// (target guid, aura id) -> open interval.
    open: HashMap<(String, u32), usize>,
    /// Player guid -> death windows (end None = still dead).
    dead: HashMap<String, Vec<(i64, Option<i64>)>>,
    hits: Vec<Hit>,
    tank_hits: Vec<(i64, String, Spell, u64)>,
    summons: Vec<Summon>,
    summon_by_guid: HashMap<String, usize>,
    /// Sated-type debuffs: (player guid, start, end).
    sated: Vec<(String, i64, Option<i64>)>,
    sated_open: HashMap<String, usize>,
    spell_names: HashMap<u32, String>,
    unit_names: HashMap<String, String>,
}

/// Everything `finish` works out, ready for the fight report.
pub(super) struct Output {
    pub uses: Vec<DefensiveUse>,
    pub spikes: Vec<Spike>,
    pub tank_hits: Vec<TankHit>,
    pub summons: Vec<EnemySummon>,
}

/// Settings `finish` needs.
pub(super) struct Fin {
    pub fight_start: i64,
    pub end_ts: i64,
    pub spike_window_ms: i64,
    pub spike_frac: f64,
    /// Group player deaths: (ts, guid).
    pub deaths: Vec<(i64, String)>,
    /// Pull windows (start, end), in order: records are numbered by these.
    pub pulls: Vec<(i64, i64)>,
}

impl Fin {
    /// 1-based pull containing `ts`.
    fn pull_at(&self, ts: i64) -> Option<usize> {
        self.pulls
            .iter()
            .position(|(s, e)| *s <= ts && ts <= *e)
            .map(|i| i + 1)
    }
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

    fn note_use(&mut self, player: &str, ab: usize, ts: i64) {
        if ability(ab).kind != AbilityKind::Defensive {
            return;
        }
        let v = self.uses.entry((player.to_string(), ab)).or_default();
        if v.last().is_none_or(|l| (ts - l).abs() > SAME_USE_MS) {
            v.push(ts);
            self.use_log.push((ts, player.to_string(), ab));
        }
    }

    pub(super) fn max_hp(&self, guid: &str) -> Option<u64> {
        self.max_hp.get(guid).copied()
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
            && i.max_hp > 0
            && self.names.contains_key(&i.guid)
        {
            self.max_hp.insert(i.guid.clone(), i.max_hp);
        }
        let ix = index();
        match &e.kind {
            Kind::Damage {
                spell,
                amount,
                absorbed,
                ..
            } => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_group_player()) else {
                    return;
                };
                let total = amount + absorbed;
                let src = e.src.as_ref();
                if let Some(s) = src
                    && let Some(&k) = self.summon_by_guid.get(&s.guid)
                {
                    self.summons[k].damage += total;
                }
                if !ctx.deferred.contains(&spell.id) {
                    if let Some(s) = src {
                        self.unit_names
                            .entry(s.guid.clone())
                            .or_insert_with(|| s.name.clone());
                    }
                    self.spell_names
                        .entry(spell.id)
                        .or_insert_with(|| spell.name.clone());
                    // HP actually lost: damage a shield soaked was already
                    // answered (the spike threshold was calibrated on this).
                    self.hits.push(Hit {
                        ts,
                        amount: *amount,
                        spell_id: spell.id,
                        src: src.map(|s| s.guid.clone()).unwrap_or_default(),
                    });
                }
                let tank = ctx
                    .specs
                    .get(&d.guid)
                    .and_then(|s| spec_info(*s))
                    .is_some_and(|(_, _, r)| r == Role::Tank);
                if tank && src.is_some_and(|s| s.is_hostile_npc()) && (ctx.tank_buster)(spell.id) {
                    self.tank_hits
                        .push((ts, d.guid.clone(), spell.clone(), total));
                }
            }
            Kind::Heal {
                amount, overheal, ..
            } => {
                if let (Some(s), Some(d)) = (&e.src, &e.dst)
                    && d.is_hostile_npc()
                    && let Some(&k) = self.summon_by_guid.get(&s.guid)
                {
                    self.summons[k].healing += amount.saturating_sub(*overheal);
                }
            }
            Kind::CastSuccess { spell, .. } => {
                let Some(src) = &e.src else { return };
                let Some(p) = self.player_of(src, ctx.owners) else {
                    return;
                };
                // Acting again after a death means they were brought back.
                if let Some(w) = self.dead.get_mut(&p).and_then(|v| v.last_mut())
                    && w.1.is_none()
                {
                    w.1 = Some(ts);
                }
                for &ab in ix.by_cast.get(&spell.id).into_iter().flatten() {
                    self.note_use(&p, ab, ts);
                    let a = ability(ab);
                    if let Some(d) = a.dur_s
                        && !a.instant
                    {
                        let target = match a.scope {
                            Some(Scope::Group) => None,
                            Some(Scope::External) => e
                                .dst
                                .as_ref()
                                .filter(|d| d.is_group_player())
                                .map(|d| d.guid.clone())
                                .or_else(|| Some(p.clone())),
                            _ => Some(p.clone()),
                        };
                        self.ivs.push(Iv {
                            target,
                            ability: ab,
                            start: ts,
                            end: Some(ts + (d * 1000.0) as i64),
                        });
                    }
                }
            }
            Kind::AuraApplied { spell, is_buff } => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_group_player()) else {
                    return;
                };
                if !is_buff {
                    if ctx.sated.contains(&spell.id) && !self.sated_open.contains_key(&d.guid) {
                        self.sated_open.insert(d.guid.clone(), self.sated.len());
                        self.sated.push((d.guid.clone(), ts, None));
                    }
                    return;
                }
                let Some(&ab) = ix.by_aura.get(&spell.id) else {
                    return;
                };
                let by = e
                    .src
                    .as_ref()
                    .and_then(|s| self.player_of(s, ctx.owners))
                    .unwrap_or_else(|| d.guid.clone());
                self.note_use(&by, ab, ts);
                let key = (d.guid.clone(), spell.id);
                if !self.open.contains_key(&key) {
                    self.open.insert(key, self.ivs.len());
                    self.ivs.push(Iv {
                        target: Some(d.guid.clone()),
                        ability: ab,
                        start: ts,
                        end: None,
                    });
                }
            }
            Kind::AuraRemoved { spell, is_buff } => {
                let Some(d) = e.dst.as_ref().filter(|d| d.is_group_player()) else {
                    return;
                };
                if !is_buff {
                    if ctx.sated.contains(&spell.id) {
                        match self.sated_open.remove(&d.guid) {
                            Some(k) => self.sated[k].2 = Some(ts),
                            // Applied before the fight started.
                            None => self.sated.push((d.guid.clone(), i64::MIN, Some(ts))),
                        }
                    }
                    return;
                }
                let Some(&ab) = ix.by_aura.get(&spell.id) else {
                    return;
                };
                match self.open.remove(&(d.guid.clone(), spell.id)) {
                    Some(k) => self.ivs[k].end = Some(ts),
                    // Up since before the fight (prepull Ironfur, Shuffle).
                    None => self.ivs.push(Iv {
                        target: Some(d.guid.clone()),
                        ability: ab,
                        start: i64::MIN,
                        end: Some(ts),
                    }),
                }
            }
            Kind::Summon { spell } => {
                if let (Some(s), Some(d)) = (&e.src, &e.dst)
                    && s.is_hostile_npc()
                    && !self.summon_by_guid.contains_key(&d.guid)
                {
                    self.summon_by_guid
                        .insert(d.guid.clone(), self.summons.len());
                    self.summons.push(Summon {
                        guid: d.guid.clone(),
                        name: d.name.clone(),
                        summoner: s.name.clone(),
                        summoner_npc: npc_id(&s.guid),
                        spell: spell.name.clone(),
                        start: ts,
                        end: None,
                        damage: 0,
                        healing: 0,
                    });
                }
            }
            Kind::Death => {
                let Some(d) = &e.dst else { return };
                if d.is_group_player() {
                    let v = self.dead.entry(d.guid.clone()).or_default();
                    if v.last().is_none_or(|w| w.1.is_some()) {
                        v.push((ts, None));
                    }
                } else if let Some(&k) = self.summon_by_guid.get(&d.guid)
                    && self.summons[k].end.is_none()
                {
                    self.summons[k].end = Some(ts);
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

    /// The cooldown to assume: the table's, or the player's own shortest
    /// observed reuse if that's shorter (talents, procs).
    fn cd_ms(&self, guid: &str, ab: usize) -> i64 {
        let table = (ability(ab).cd_s * 1000.0) as i64;
        self.uses
            .get(&(guid.to_string(), ab))
            .into_iter()
            .flat_map(|u| u.windows(2).map(|w| w[1] - w[0]))
            .filter(|g| *g >= MIN_OBSERVED_CD_MS)
            .min()
            .unwrap_or(table)
            .min(table)
    }

    /// Was this defensive ready at `t`: used earlier in the fight, off
    /// cooldown, player alive? Returns the last use.
    fn ready(&self, guid: &str, ab: usize, t: i64) -> Option<i64> {
        let last = *self
            .uses
            .get(&(guid.to_string(), ab))?
            .iter()
            .rev()
            .find(|u| **u < t)?;
        (last + self.cd_ms(guid, ab) <= t && self.alive(guid, t)).then_some(last)
    }

    /// Buffs (defensive or mitigation) up on `guid` at `t`.
    fn active_on(&self, guid: &str, t: i64, end_ts: i64) -> impl Iterator<Item = &Iv> {
        self.ivs.iter().filter(move |iv| {
            iv.start <= t
                && t <= iv.end.unwrap_or(end_ts)
                && iv.target.as_deref().is_none_or(|g| g == guid)
        })
    }

    /// Group players seen in this fight, by name.
    fn group(&self, specs: &HashMap<String, u32>) -> Vec<String> {
        let mut g: Vec<String> = self
            .names
            .keys()
            .filter(|k| specs.contains_key(*k))
            .cloned()
            .collect();
        g.sort_by_key(|k| self.name(k));
        g
    }

    /// Ready defensives of `guid` at `t`, of the given scope.
    fn ready_list(
        &self,
        guid: &str,
        t: i64,
        scope: Scope,
        fight_start: i64,
    ) -> Vec<ReadyDefensive> {
        let mut out: Vec<ReadyDefensive> = self
            .uses
            .keys()
            .filter(|(g, ab)| g == guid && ability(*ab).scope == Some(scope))
            .filter_map(|(_, ab)| {
                self.ready(guid, *ab, t).map(|last| ReadyDefensive {
                    player: self.name(guid),
                    ability: ability(*ab).name.clone(),
                    last_used_s: secs(last - fight_start),
                })
            })
            .collect();
        out.sort_by(|a, b| a.ability.cmp(&b.ability));
        out
    }

    /// For a player's death: personal defensives ready a moment before it,
    /// and the buffs that were up.
    pub(super) fn at_death(
        &self,
        guid: &str,
        ts: i64,
        fight_start: i64,
    ) -> (Vec<ReadyDefensive>, Vec<String>) {
        let ready = self.ready_list(guid, ts - DEATH_REACTION_MS, Scope::Personal, fight_start);
        let mut active: Vec<String> = self
            .active_on(guid, ts, ts)
            .map(|iv| ability(iv.ability).name.clone())
            .collect();
        active.sort();
        active.dedup();
        (ready, active)
    }

    /// Lust over [start, end]: was it available at `start`, for how many ms,
    /// given who's in the group, their Sated debuffs, and the lust casts seen.
    pub(super) fn lust_window(
        &self,
        start: i64,
        end: i64,
        specs: &HashMap<String, u32>,
        lust_casts: &[i64],
    ) -> (bool, i64) {
        let group = self.group(specs);
        let capable = group.iter().any(|g| {
            specs
                .get(g)
                .and_then(|s| spec_info(*s))
                .is_some_and(|(c, _, _)| LUST_CLASSES.contains(&c))
        });
        if !capable || end <= start {
            return (false, 0);
        }
        let mut all: Vec<(i64, i64)> = self
            .sated
            .iter()
            .filter(|(g, _, _)| group.contains(g))
            .map(|(_, s, e)| (*s, e.unwrap_or(i64::MAX)))
            .collect();
        for t in lust_casts {
            if !all.iter().any(|(s, _)| (s - t).abs() <= SATED_MATCH_MS) {
                all.push((*t, t + SATED_MS));
            }
        }
        let at_start = !all.iter().any(|(s, e)| *s <= start && start < *e);
        let mut sated: Vec<(i64, i64)> = all
            .iter()
            .map(|(s, e)| ((*s).max(start), (*e).min(end)))
            .filter(|(s, e)| s < e)
            .collect();
        sated.sort_unstable();
        let mut covered = 0;
        let mut cur: Option<(i64, i64)> = None;
        for (s, e) in sated {
            match &mut cur {
                Some(c) if s <= c.1 => c.1 = c.1.max(e),
                _ => {
                    if let Some(c) = cur {
                        covered += c.1 - c.0;
                    }
                    cur = Some((s, e));
                }
            }
        }
        if let Some(c) = cur {
            covered += c.1 - c.0;
        }
        (at_start, end - start - covered)
    }

    pub(super) fn finish(&self, fin: &Fin, specs: &HashMap<String, u32>) -> Output {
        let start = fin.fight_start;
        let rel = |t: i64| secs(t - start);
        let group = self.group(specs);

        let uses = self
            .use_log
            .iter()
            .map(|(ts, g, ab)| DefensiveUse {
                player: self.name(g),
                ability: ability(*ab).name.clone(),
                scope: ability(*ab)
                    .scope
                    .map(|s| format!("{s:?}").to_lowercase())
                    .unwrap_or_default(),
                at_s: rel(*ts),
                pull: fin.pull_at(*ts),
            })
            .collect();

        let tank_hits = self
            .tank_hits
            .iter()
            .map(|(ts, tank, spell, amount)| {
                let mut mitigation = vec![];
                let mut defensives = vec![];
                for iv in self.active_on(tank, *ts, fin.end_ts) {
                    let a = ability(iv.ability);
                    if !a.protects_against(spell) {
                        continue;
                    }
                    match a.kind {
                        AbilityKind::Mitigation => mitigation.push(a.name.clone()),
                        _ => defensives.push(a.name.clone()),
                    }
                }
                for v in [&mut mitigation, &mut defensives] {
                    v.sort();
                    v.dedup();
                }
                TankHit {
                    pull: fin.pull_at(*ts),
                    at_s: rel(*ts),
                    tank: self.name(tank),
                    spell_id: spell.id,
                    name: spell.name.clone(),
                    school: spell.school,
                    amount: *amount,
                    mitigation,
                    defensives,
                }
            })
            .collect();

        let summons = self
            .summons
            .iter()
            .map(|s| EnemySummon {
                pull: fin.pull_at(s.start),
                name: s.name.clone(),
                npc: npc_id(&s.guid),
                summoner: s.summoner.clone(),
                summoner_npc: s.summoner_npc,
                spell: s.spell.clone(),
                start_s: rel(s.start),
                end_s: rel(s.end.unwrap_or(fin.end_ts)),
                killed: s.end.is_some(),
                damage_to_group: s.damage,
                healing_to_enemies: s.healing,
            })
            .collect();

        Output {
            uses,
            spikes: self.spikes(fin, &group),
            tank_hits,
            summons,
        }
    }

    fn spikes(&self, fin: &Fin, group: &[String]) -> Vec<Spike> {
        let ghp: u64 = group.iter().filter_map(|g| self.max_hp(g)).sum();
        if ghp == 0 || self.hits.is_empty() {
            return vec![];
        }
        let thr = fin.spike_frac * ghp as f64;
        // Sliding window over group damage taken; windows over the threshold
        // become (start, end, peak) and overlapping ones merge.
        let mut found: Vec<(usize, usize, u64)> = vec![];
        let (mut l, mut sum) = (0usize, 0u64);
        for r in 0..self.hits.len() {
            sum += self.hits[r].amount;
            while self.hits[l].ts < self.hits[r].ts - fin.spike_window_ms {
                sum -= self.hits[l].amount;
                l += 1;
            }
            if sum as f64 >= thr {
                match found.last_mut() {
                    Some(f) if self.hits[l].ts <= self.hits[f.1].ts => {
                        f.1 = r;
                        f.2 = f.2.max(sum);
                    }
                    _ => found.push((l, r, sum)),
                }
            }
        }
        let start = fin.fight_start;
        let rel = |t: i64| secs(t - start);
        found
            .into_iter()
            .map(|(l, r, peak)| {
                let (s, e) = (self.hits[l].ts, self.hits[r].ts);
                let hits = &self.hits[l..=r];
                // Top spells by (spell, source name); remember summons among the sources.
                let mut by: BTreeMap<(u32, String), (u64, String, Option<usize>)> = BTreeMap::new();
                for h in hits {
                    let src_name = self
                        .unit_names
                        .get(&h.src)
                        .cloned()
                        .unwrap_or_else(|| "Environment".into());
                    let x = by
                        .entry((h.spell_id, src_name))
                        .or_insert((0, h.src.clone(), None));
                    x.0 += h.amount;
                    if let Some(&k) = self.summon_by_guid.get(&h.src) {
                        let older =
                            x.2.is_none_or(|o| self.summons[k].start < self.summons[o].start);
                        if older {
                            x.2 = Some(k);
                        }
                    }
                }
                let mut top: Vec<SpikeSpell> = by
                    .into_iter()
                    .map(|((id, source), (damage, guid, summon))| SpikeSpell {
                        spell_id: id,
                        name: self
                            .spell_names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Spell {id}")),
                        source,
                        source_npc: npc_id(&guid),
                        damage,
                        summoned_by: summon.map(|k| self.summons[k].summoner.clone()),
                        summon_age_s: summon.map(|k| secs((s - self.summons[k].start).max(0))),
                    })
                    .collect();
                top.sort_by(|a, b| {
                    b.damage
                        .cmp(&a.damage)
                        .then(a.spell_id.cmp(&b.spell_id))
                        .then_with(|| a.source.cmp(&b.source))
                });
                top.truncate(SPIKE_TOP);
                let deaths = fin
                    .deaths
                    .iter()
                    .filter(|(t, _)| *t >= s && *t <= e + SPIKE_DEATH_AFTER_MS)
                    .map(|(_, g)| self.name(g))
                    .collect();
                let is_group = |ab: usize| {
                    let a = ability(ab);
                    a.kind == AbilityKind::Defensive && a.scope == Some(Scope::Group)
                };
                let group_used: Vec<GroupDefensive> = self
                    .use_log
                    .iter()
                    .filter(|(t, _, ab)| is_group(*ab) && *t >= s - SPIKE_LEAD_MS && *t <= e)
                    .map(|(t, g, ab)| GroupDefensive {
                        player: self.name(g),
                        ability: ability(*ab).name.clone(),
                        at_s: rel(*t),
                    })
                    .collect();
                let group_ready = group
                    .iter()
                    .flat_map(|g| self.ready_list(g, s, Scope::Group, start))
                    .filter(|r| {
                        !group_used
                            .iter()
                            .any(|u| u.player == r.player && u.ability == r.ability)
                    })
                    .collect();
                Spike {
                    pull: fin.pull_at(s),
                    start_s: rel(s),
                    end_s: rel(e),
                    damage: hits.iter().map(|h| h.amount).sum(),
                    peak_frac: peak as f64 / ghp as f64,
                    top,
                    deaths,
                    group_used,
                    group_ready,
                }
            })
            .collect()
    }
}
