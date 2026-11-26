//! Segments the event stream into fights (raid encounters / M+ runs),
//! then aggregates group- and player-level metrics for each.

use crate::config::Config;
use crate::model::*;
use crate::specs::{Role, spec_info};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

// ---------------------------------------------------------------- reports

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum FightKind {
    Raid {
        encounter_id: u32,
        name: String,
        difficulty: String,
        group_size: u32,
        success: bool,
        boss_hp_pct: Option<f64>,
    },
    MythicPlus {
        zone: String,
        map_id: u32,
        level: u32,
        affixes: Vec<u32>,
        success: bool,
        time_s: f64,
        bosses: Vec<BossMarker>,
    },
}

#[derive(Serialize, Clone, Debug)]
pub struct BossMarker {
    pub name: String,
    /// `ENCOUNTER_START` ID as logged.
    pub encounter_id: u32,
    pub start_s: f64,
    pub end_s: f64,
    pub success: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct RecapLine {
    pub at_s: f64,
    pub spell: String,
    pub source: String,
    pub amount: u64,
    pub hp_pct_after: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeathReport {
    pub player: String,
    pub at_s: f64,
    pub killing_blow: String,
    pub killing_spell_id: u32,
    pub recap: Vec<RecapLine>,
    pub damage_last_5s: u64,
    pub healthstone_used_before: bool,
    pub potion_used_before: bool,
    pub pull: Option<usize>,
    /// Raid wipe: died after the wipe was effectively called (cascade noise).
    pub wipe_cascade: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct PullReport {
    pub index: usize,
    pub start_s: f64,
    pub end_s: f64,
    pub deaths: u32,
    pub enemies_killed: u32,
    pub boss: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct EnemyCastReport {
    pub spell_id: u32,
    pub name: String,
    pub started: u32,
    pub succeeded: u32,
    pub interrupted: u32,
    pub priority: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct EnemySpellReport {
    pub spell_id: u32,
    pub name: String,
    pub total: u64,
    pub hits: u32,
    pub players_hit: u32,
    pub non_tank_hit_frac: f64,
    pub max_hits_per_player: u32,
    pub median_hits_per_player: f64,
    pub configured_avoidable: bool,
    pub likely_avoidable: bool,
    pub per_player: Vec<(String, u64, u32)>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlayerReport {
    pub name: String,
    pub guid: String,
    pub class: String,
    pub spec: String,
    pub role: Role,
    pub role_inferred: bool,
    pub damage_done: u64,
    pub dps: f64,
    /// Effective healing, including shields and heal-absorb removal.
    pub healing_done: u64,
    /// Portion of healing_done that came from shields/absorbs.
    pub absorbs_done: u64,
    pub hps: f64,
    pub overheal_pct: f64,
    pub damage_taken: u64,
    pub avoidable_taken: u64,
    pub likely_avoidable_taken: u64,
    pub interrupts: u32,
    pub dispels: u32,
    pub deaths: u32,
    pub casts: u32,
    pub activity_pct: f64,
    pub healthstones: u32,
    pub potions: u32,
    pub top_damage_taken: Vec<(String, u64, u32)>,
    pub wcl_rank_percent: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Suggestion {
    /// "group" or a player name
    pub scope: String,
    pub severity: Severity,
    pub category: String,
    pub message: String,
    pub evidence: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    High,
    Medium,
    Low,
}

/// Everything seen about one hostile spell in a fight, for `season suggest`.
/// Raw counts only; judging them is the suggester's job.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SpellEvidence {
    pub spell_id: u32,
    pub name: String,
    /// Boss fight it was mostly seen in (log name); None = trash.
    pub boss: Option<String>,
    /// Cast bars started / completed / interrupted.
    pub casts_started: u32,
    pub casts_succeeded: u32,
    pub interrupted: u32,
    /// Casts without a cast bar.
    pub instant_casts: u32,
    /// Damage to group players.
    pub hits: u32,
    pub damage: u64,
    pub players_hit: u32,
    pub tank_damage: u64,
    pub non_tank_hit_frac: f64,
    pub likely_avoidable: bool,
    /// Harmful auras applied to group players, and how many were dispelled.
    pub applied: u32,
    pub dispelled: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct FightReport {
    #[serde(skip)]
    #[cfg_attr(not(feature = "wcl"), allow(dead_code))]
    pub start_ts: i64,
    pub title: String,
    pub kind: FightKind,
    pub duration_s: f64,
    pub complete: bool,
    pub players: Vec<PlayerReport>,
    pub deaths: Vec<DeathReport>,
    pub pulls: Vec<PullReport>,
    pub downtime_s: f64,
    pub enemy_casts: Vec<EnemyCastReport>,
    pub enemy_damage: Vec<EnemySpellReport>,
    pub lust_at_s: Vec<f64>,
    pub suggestions: Vec<Suggestion>,
    /// Per hostile spell, for `season suggest`. Not part of the report output.
    #[serde(skip)]
    pub spell_evidence: Vec<SpellEvidence>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProgressionReport {
    pub encounter_id: u32,
    pub name: String,
    pub difficulty: String,
    pub attempts: u32,
    pub kills: u32,
    pub best_hp_pct: Option<f64>,
    pub avg_duration_s: f64,
    pub death_causes: Vec<(String, u32)>,
    pub first_death_causes: Vec<(String, u32)>,
    pub frequent_deaths: Vec<(String, u32)>,
    pub suggestions: Vec<Suggestion>,
}

// --------------------------------------------------------------- builders

#[derive(Default)]
struct Hit {
    ts: i64,
    spell: String,
    spell_id: u32,
    src: String,
    amount: u64,
    hp_pct: Option<f64>,
}

#[derive(Default)]
struct PS {
    name: String,
    damage: u64,
    healing: u64,
    absorbs: u64,
    overheal: u64,
    taken: u64,
    swing_taken: u64,
    taken_by_spell: HashMap<u32, (String, u64, u32)>,
    interrupts: u32,
    dispels: u32,
    casts: u32,
    pending_start: Option<i64>,
    intervals: Vec<(i64, i64)>,
    dead_since: Option<i64>,
    dead_windows: Vec<(i64, i64)>,
    healthstones: Vec<i64>,
    potions: Vec<i64>,
    recent: VecDeque<Hit>,
    deaths: u32,
}

#[derive(Default)]
struct EC {
    name: String,
    started: u32,
    success: u32,
    interrupted: u32,
}

/// Evidence counters not covered by EC/ES, plus where the spell was seen.
#[derive(Default)]
struct EX {
    name: String,
    instant: u32,
    applied: u32,
    dispelled: u32,
    /// Boss context (None = trash) -> times seen.
    ctx: HashMap<Option<String>, u32>,
}

#[derive(Default)]
struct ES {
    name: String,
    total: u64,
    hits: u32,
    per_player: HashMap<String, (u64, u32)>,
}

struct Pull {
    start: i64,
    end: i64,
    deaths: u32,
    kills: u32,
    boss: Option<String>,
}

enum Seed {
    Raid {
        id: u32,
        name: String,
        difficulty: u32,
        size: u32,
    },
    MPlus {
        zone: String,
        map_id: u32,
        level: u32,
        affixes: Vec<u32>,
    },
}

struct FightBuilder {
    seed: Seed,
    start: i64,
    last_ts: i64,
    players: HashMap<String, PS>,
    enemy_casts: HashMap<u32, EC>,
    enemy_dmg: HashMap<u32, ES>,
    boss_track: Option<(String, u64, u64)>,
    lust: Vec<i64>,
    pulls: Vec<Pull>,
    cur_pull: Option<Pull>,
    bosses: Vec<BossMarker>,
    /// M+ boss fight in progress: (name, start ts, encounter ID).
    open_boss: Option<(String, i64, u32)>,
    enemy_extra: HashMap<u32, EX>,
    deaths: Vec<DeathReport>,
    /// hostile cast bars in progress: (caster guid, spell id) -> start ts
    enemy_pending: HashMap<(String, u32), i64>,
}

pub fn difficulty_name(id: u32) -> String {
    match id {
        1 => "Normal",
        2 => "Heroic",
        8 => "Mythic+",
        14 => "Normal",
        15 => "Heroic",
        16 => "Mythic",
        17 => "LFR",
        23 => "Mythic",
        205 => "Follower",
        208 => "Delve",
        220 => "Story",
        _ => return format!("Difficulty {id}"),
    }
    .to_string()
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

fn merge(mut v: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    v.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn measure(v: &[(i64, i64)]) -> i64 {
    v.iter().map(|(a, b)| b - a).sum()
}

fn intersect(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let (mut i, mut j, mut out) = (0, 0, vec![]);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo < hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 { i += 1 } else { j += 1 }
    }
    out
}

fn subtract(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = vec![];
    for &(mut s, e) in a {
        for &(bs, be) in b {
            if be <= s || bs >= e {
                continue;
            }
            if bs > s {
                out.push((s, bs));
            }
            s = s.max(be);
        }
        if s < e {
            out.push((s, e));
        }
    }
    out
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

impl FightBuilder {
    fn new(seed: Seed, ts: i64) -> Self {
        FightBuilder {
            seed,
            start: ts,
            last_ts: ts,
            players: HashMap::new(),
            enemy_casts: HashMap::new(),
            enemy_dmg: HashMap::new(),
            boss_track: None,
            lust: vec![],
            pulls: vec![],
            cur_pull: None,
            bosses: vec![],
            open_boss: None,
            enemy_extra: HashMap::new(),
            deaths: vec![],
            enemy_pending: HashMap::new(),
        }
    }

    fn ps(&mut self, guid: &str, name: Option<&str>) -> &mut PS {
        let p = self.players.entry(guid.to_string()).or_default();
        if p.name.is_empty()
            && let Some(n) = name
        {
            p.name = n.to_string();
        }
        p
    }

    /// Resolve a unit to the group player responsible for it (self or pet owner).
    fn attrib(
        u: &Option<Unit>,
        owners: &HashMap<String, String>,
    ) -> Option<(String, Option<String>)> {
        let u = u.as_ref()?;
        if u.is_group_player() {
            return Some((u.guid.clone(), Some(u.name.clone())));
        }
        if !u.is_hostile() {
            let mut g = owners.get(&u.guid)?;
            // follow guardian -> pet -> player chains
            for _ in 0..3 {
                if g.starts_with("Player-") {
                    return Some((g.clone(), None));
                }
                g = owners.get(g)?;
            }
        }
        None
    }

    fn touch_pull(&mut self, ts: i64, gap: i64) {
        match &mut self.cur_pull {
            Some(p) if ts - p.end <= gap => p.end = ts,
            _ => {
                if let Some(p) = self.cur_pull.take() {
                    self.pulls.push(p);
                }
                self.cur_pull = Some(Pull {
                    start: ts,
                    end: ts,
                    deaths: 0,
                    kills: 0,
                    boss: self.open_boss.as_ref().map(|b| b.0.clone()),
                });
            }
        }
    }

    /// Note a hostile spell for the evidence table, with the boss context.
    fn seen(&mut self, id: u32, name: &str) -> &mut EX {
        let ctx = match (&self.seed, &self.open_boss) {
            (Seed::Raid { name, .. }, _) => Some(name.clone()),
            (Seed::MPlus { .. }, Some(b)) => Some(b.0.clone()),
            (Seed::MPlus { .. }, None) => None,
        };
        let x = self.enemy_extra.entry(id).or_default();
        if x.name.is_empty() {
            x.name = name.to_string();
        }
        *x.ctx.entry(ctx).or_default() += 1;
        x
    }

    fn ingest(&mut self, e: &Event, owners: &HashMap<String, String>, cfg: &Config) {
        let ts = e.ts;
        self.last_ts = ts;
        let g = &cfg.general;
        match &e.kind {
            Kind::Damage {
                spell,
                amount,
                overkill,
                absorbed,
                info,
                ..
            } => {
                let src_p = Self::attrib(&e.src, owners);
                let dst_group = e.dst.as_ref().is_some_and(|d| d.is_group_player());
                let src_hostile = e.src.as_ref().is_some_and(|s| s.is_hostile_npc());
                let dst_hostile = e.dst.as_ref().is_some_and(|d| d.is_hostile_npc());

                if (src_p.is_some() && dst_hostile) || (src_hostile && dst_group) {
                    self.touch_pull(ts, g.pull_gap_ms);
                }
                if let (Some((pg, pn)), true) = (&src_p, dst_hostile) {
                    let eff = (amount + absorbed).saturating_sub(*overkill);
                    self.ps(pg, pn.as_deref()).damage += eff;
                }
                if let (Some(d), Some(i)) = (&e.dst, info)
                    && dst_hostile
                    && i.guid == d.guid
                {
                    let bigger = self.boss_track.as_ref().is_none_or(|b| i.max_hp >= b.2);
                    if bigger {
                        self.boss_track = Some((d.guid.clone(), i.hp, i.max_hp));
                    } else if let Some(b) = &mut self.boss_track
                        && b.0 == d.guid
                    {
                        b.1 = i.hp;
                    }
                }
                if dst_group {
                    let d = e.dst.as_ref().unwrap();
                    let total = amount + absorbed;
                    let src_name = e
                        .src
                        .as_ref()
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "Environment".into());
                    let hp_pct = info
                        .as_ref()
                        .filter(|i| i.guid == d.guid && i.max_hp > 0)
                        .map(|i| 100.0 * i.hp as f64 / i.max_hp as f64);
                    let deferred = g.deferred_damage_spell_ids.contains(&spell.id);
                    let p = self.ps(&d.guid, Some(&d.name));
                    if !deferred {
                        p.taken += total;
                    }
                    if spell.id == 1 && src_hostile {
                        p.swing_taken += total;
                    }
                    if !deferred {
                        let t = p
                            .taken_by_spell
                            .entry(spell.id)
                            .or_insert_with(|| (spell.name.clone(), 0, 0));
                        t.1 += total;
                        t.2 += 1;
                    }
                    p.recent.push_back(Hit {
                        ts,
                        spell: spell.name.clone(),
                        spell_id: spell.id,
                        src: src_name,
                        amount: total,
                        hp_pct,
                    });
                    while p.recent.front().is_some_and(|h| ts - h.ts > 10_000) {
                        p.recent.pop_front();
                    }
                    if src_hostile && spell.id > 1 {
                        self.seen(spell.id, &spell.name);
                    }
                    if src_hostile || e.src.is_none() {
                        let es = self.enemy_dmg.entry(spell.id).or_default();
                        es.name.clone_from(&spell.name);
                        es.total += total;
                        es.hits += 1;
                        let pp = es.per_player.entry(d.guid.clone()).or_default();
                        pp.0 += total;
                        pp.1 += 1;
                    }
                }
            }
            Kind::Heal {
                amount,
                overheal,
                absorbed,
                ..
            } => {
                // `amount` includes overheal but excludes the part eaten by a
                // heal-absorb debuff; that part still counts as effective healing.
                if let Some((pg, pn)) = Self::attrib(&e.src, owners)
                    && e.dst.as_ref().is_some_and(|d| !d.is_hostile())
                {
                    let p = self.ps(&pg, pn.as_deref());
                    p.healing += amount.saturating_sub(*overheal) + absorbed;
                    p.overheal += overheal;
                }
            }
            Kind::Absorbed {
                absorber,
                spell,
                amount,
            } => {
                if g.absorb_exclude_spell_ids.contains(&spell.id) {
                    return;
                }
                // Credit shields to whoever cast them (players or their pets),
                // only when protecting friendly units.
                if e.dst.as_ref().is_some_and(|d| !d.is_hostile())
                    && let Some((pg, pn)) = Self::attrib(&Some(absorber.clone()), owners)
                {
                    let p = self.ps(&pg, pn.as_deref());
                    p.healing += amount;
                    p.absorbs += amount;
                }
            }
            Kind::CastStart { spell } => {
                if let Some(s) = &e.src {
                    if s.is_group_player() {
                        self.ps(&s.guid, Some(&s.name)).pending_start = Some(ts);
                    } else if s.is_hostile_npc() {
                        let ec = self.enemy_casts.entry(spell.id).or_default();
                        ec.name.clone_from(&spell.name);
                        ec.started += 1;
                        self.enemy_pending.insert((s.guid.clone(), spell.id), ts);
                        self.seen(spell.id, &spell.name);
                    }
                }
            }
            Kind::CastSuccess { spell, .. } => {
                // Lust can come from a pet (hunter Primal Rage), so check ownership.
                if g.lust_spell_ids.contains(&spell.id)
                    && Self::attrib(&e.src, owners).is_some()
                    && self.lust.last().is_none_or(|l| ts - l > 60_000)
                {
                    self.lust.push(ts);
                }
                if let Some(s) = &e.src {
                    if s.is_group_player() {
                        let gcd = g.gcd_ms;
                        let p = self.ps(&s.guid, Some(&s.name));
                        let st = p
                            .pending_start
                            .take()
                            .filter(|st| ts - st < 6000)
                            .unwrap_or(ts);
                        p.intervals.push((st, ts + gcd));
                        p.casts += 1;
                        if let Some(ds) = p.dead_since.take() {
                            p.dead_windows.push((ds, ts));
                        }
                        if g.healthstone_spell_ids.contains(&spell.id) {
                            p.healthstones.push(ts);
                        }
                        if g.potion_spell_ids.contains(&spell.id) {
                            p.potions.push(ts);
                        }
                    } else if s.is_hostile_npc() {
                        // Only casts with a visible cast bar could have been kicked;
                        // instant versions of the same spell are ignored.
                        let started = self
                            .enemy_pending
                            .remove(&(s.guid.clone(), spell.id))
                            .is_some_and(|st| ts - st <= 15_000);
                        if started {
                            let ec = self.enemy_casts.entry(spell.id).or_default();
                            ec.name.clone_from(&spell.name);
                            ec.success += 1;
                        } else {
                            self.seen(spell.id, &spell.name).instant += 1;
                        }
                    }
                }
            }
            Kind::CastFailed { .. } => {
                if let Some(s) = e.src.as_ref().filter(|s| s.is_group_player()) {
                    let p = self.ps(&s.guid, Some(&s.name));
                    if let Some(st) = p.pending_start.take() {
                        p.intervals.push((st, ts));
                    }
                }
            }
            Kind::Interrupt { extra, .. } => {
                if let Some((pg, pn)) = Self::attrib(&e.src, owners) {
                    self.ps(&pg, pn.as_deref()).interrupts += 1;
                }
                if let Some(d) = e.dst.as_ref().filter(|d| d.is_hostile_npc()) {
                    self.enemy_pending.remove(&(d.guid.clone(), extra.id));
                    let ec = self.enemy_casts.entry(extra.id).or_default();
                    ec.name.clone_from(&extra.name);
                    ec.interrupted += 1;
                    self.seen(extra.id, &extra.name);
                }
            }
            Kind::Dispel { extra, .. } => {
                if let Some((pg, pn)) = Self::attrib(&e.src, owners)
                    && e.dst.as_ref().is_some_and(|d| !d.is_hostile())
                {
                    self.ps(&pg, pn.as_deref()).dispels += 1;
                    if e.dst.as_ref().is_some_and(|d| d.is_group_player()) {
                        self.seen(extra.id, &extra.name).dispelled += 1;
                    }
                }
            }
            Kind::AuraApplied {
                spell,
                is_buff: false,
            } => {
                if e.src.as_ref().is_some_and(|s| s.is_hostile_npc())
                    && e.dst.as_ref().is_some_and(|d| d.is_group_player())
                {
                    self.seen(spell.id, &spell.name).applied += 1;
                }
            }
            Kind::Death => {
                let Some(d) = &e.dst else { return };
                if d.is_group_player() {
                    let rel_start = self.start;
                    let pull_idx = self.cur_pull.as_ref().map(|_| self.pulls.len());
                    if let Some(cp) = &mut self.cur_pull {
                        cp.deaths += 1;
                    }
                    let p = self.ps(&d.guid, Some(&d.name));
                    if p.dead_since.is_some() {
                        return; // duplicate (e.g. feign/spirit of redemption)
                    }
                    p.dead_since = Some(ts);
                    p.deaths += 1;
                    let kb = p.recent.back();
                    let recap: Vec<RecapLine> = p
                        .recent
                        .iter()
                        .rev()
                        .take(6)
                        .rev()
                        .map(|h| RecapLine {
                            at_s: secs(h.ts - rel_start),
                            spell: h.spell.clone(),
                            source: h.src.clone(),
                            amount: h.amount,
                            hp_pct_after: h.hp_pct,
                        })
                        .collect();
                    let report = DeathReport {
                        player: d.name.clone(),
                        at_s: secs(ts - rel_start),
                        killing_blow: kb
                            .map(|h| format!("{} ({})", h.spell, h.src))
                            .unwrap_or_else(|| "Unknown".into()),
                        killing_spell_id: kb.map(|h| h.spell_id).unwrap_or(0),
                        damage_last_5s: p
                            .recent
                            .iter()
                            .filter(|h| ts - h.ts <= 5000)
                            .map(|h| h.amount)
                            .sum(),
                        healthstone_used_before: !p.healthstones.is_empty(),
                        potion_used_before: !p.potions.is_empty(),
                        recap,
                        pull: pull_idx,
                        wipe_cascade: false,
                    };
                    p.recent.clear();
                    self.deaths.push(report);
                } else if d.is_hostile_npc()
                    && let Some(cp) = &mut self.cur_pull
                {
                    cp.kills += 1;
                }
            }
            _ => {}
        }
    }

    fn finish(
        mut self,
        end_ts: i64,
        end: EndInfo,
        cfg: &Config,
        specs: &HashMap<String, u32>,
    ) -> FightReport {
        if let Some(p) = self.cur_pull.take() {
            self.pulls.push(p);
        }
        let start = self.start;
        let end_ts = end_ts.max(self.last_ts);
        let dur_ms = (end_ts - start).max(1);
        let dur = secs(dur_ms);
        let is_mplus = matches!(self.seed, Seed::MPlus { .. });

        let windows: Vec<(i64, i64)> = if is_mplus {
            merge(
                self.pulls
                    .iter()
                    .map(|p| (p.start, p.end + cfg.general.gcd_ms))
                    .collect(),
            )
        } else {
            vec![(start, end_ts)]
        };
        let combat_ms = measure(&windows);
        let downtime_s = if is_mplus {
            secs(dur_ms - combat_ms).max(0.0)
        } else {
            0.0
        };

        // roles
        let mut players: Vec<PlayerReport> = vec![];
        let swing_vals: Vec<u64> = self.players.values().map(|p| p.swing_taken).collect();
        let max_swing = swing_vals.iter().copied().max().unwrap_or(0);
        for (guid, p) in self.players.iter_mut() {
            if p.name.is_empty() {
                continue;
            }
            if let Some(ds) = p.dead_since {
                p.dead_windows.push((ds, end_ts));
            }
            let (class, spec, role, inferred) = match specs.get(guid).and_then(|s| spec_info(*s)) {
                Some((c, s, r)) => (c.to_string(), s.to_string(), r, false),
                None => {
                    let r = if p.healing > p.damage && p.healing > 0 {
                        Role::Healer
                    } else if max_swing > 0 && p.swing_taken == max_swing && p.swing_taken > 0 {
                        Role::Tank
                    } else {
                        Role::Dps
                    };
                    ("Unknown".into(), "Unknown".into(), r, true)
                }
            };
            let alive = subtract(&windows, &merge(p.dead_windows.clone()));
            let active = intersect(&merge(std::mem::take(&mut p.intervals)), &alive);
            let alive_ms = measure(&alive);
            let activity = if alive_ms > 0 {
                measure(&active) as f64 / alive_ms as f64
            } else {
                0.0
            };
            let avoidable_taken = p
                .taken_by_spell
                .iter()
                .filter(|(id, _)| cfg.avoidable_note(**id).is_some())
                .map(|(_, v)| v.1)
                .sum();
            let mut top: Vec<(String, u64, u32)> = p.taken_by_spell.values().cloned().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            top.truncate(5);
            let total_heal = p.healing + p.overheal;
            players.push(PlayerReport {
                name: p.name.clone(),
                guid: guid.clone(),
                class,
                spec,
                role,
                role_inferred: inferred,
                damage_done: p.damage,
                dps: p.damage as f64 / dur,
                healing_done: p.healing,
                absorbs_done: p.absorbs,
                hps: p.healing as f64 / dur,
                overheal_pct: if total_heal > 0 {
                    p.overheal as f64 / total_heal as f64
                } else {
                    0.0
                },
                damage_taken: p.taken,
                avoidable_taken,
                likely_avoidable_taken: 0,
                interrupts: p.interrupts,
                dispels: p.dispels,
                deaths: p.deaths,
                casts: p.casts,
                activity_pct: activity.min(1.0),
                healthstones: p.healthstones.len() as u32,
                potions: p.potions.len() as u32,
                top_damage_taken: top,
                wcl_rank_percent: None,
            });
        }
        players.sort_by(|a, b| {
            (a.role as u8)
                .cmp(&(b.role as u8))
                .then(b.damage_done.cmp(&a.damage_done))
                .then_with(|| a.name.cmp(&b.name))
        });

        // enemy damage + avoidable heuristic
        let group_taken: u64 = players.iter().map(|p| p.damage_taken).sum();
        let non_tanks: Vec<&PlayerReport> =
            players.iter().filter(|p| p.role != Role::Tank).collect();
        let name_of: HashMap<&str, &str> = players
            .iter()
            .map(|p| (p.guid.as_str(), p.name.as_str()))
            .collect();
        let mut enemy_damage: Vec<EnemySpellReport> = self
            .enemy_dmg
            .iter()
            .map(|(id, es)| {
                let nt_hit = non_tanks
                    .iter()
                    .filter(|p| es.per_player.contains_key(&p.guid))
                    .count();
                let frac = if non_tanks.is_empty() {
                    0.0
                } else {
                    nt_hit as f64 / non_tanks.len() as f64
                };
                let mut hits: Vec<f64> = es.per_player.values().map(|v| v.1 as f64).collect();
                let max_hits = hits.iter().cloned().fold(0.0, f64::max);
                let med = median(&mut hits);
                let share = if group_taken > 0 {
                    es.total as f64 / group_taken as f64
                } else {
                    0.0
                };
                let configured = cfg.avoidable_note(*id).is_some();
                let likely = !configured
                    && *id > 1
                    && share >= cfg.general.heuristic_min_share
                    && es.hits >= 2
                    && nt_hit >= 1
                    && (frac <= 0.6 || (max_hits >= 4.0 && max_hits >= 3.0 * med));
                let mut pp: Vec<(String, u64, u32)> = es
                    .per_player
                    .iter()
                    .map(|(g, v)| {
                        (
                            name_of.get(g.as_str()).unwrap_or(&"?").to_string(),
                            v.0,
                            v.1,
                        )
                    })
                    .collect();
                pp.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                EnemySpellReport {
                    spell_id: *id,
                    name: es.name.clone(),
                    total: es.total,
                    hits: es.hits,
                    players_hit: es.per_player.len() as u32,
                    non_tank_hit_frac: frac,
                    max_hits_per_player: max_hits as u32,
                    median_hits_per_player: med,
                    configured_avoidable: configured,
                    likely_avoidable: likely,
                    per_player: pp,
                }
            })
            .collect();
        enemy_damage.sort_by(|a, b| b.total.cmp(&a.total).then(a.spell_id.cmp(&b.spell_id)));
        for p in players.iter_mut() {
            p.likely_avoidable_taken = enemy_damage
                .iter()
                .filter(|s| s.likely_avoidable)
                .filter_map(|s| s.per_player.iter().find(|x| x.0 == p.name).map(|x| x.1))
                .sum();
        }

        // Different mobs often share a spell name with different IDs: merge by name.
        // Merge in spell-ID order so the reported ID doesn't depend on HashMap order.
        let mut cast_ids: Vec<(&u32, &EC)> = self
            .enemy_casts
            .iter()
            .filter(|(id, ec)| {
                ec.interrupted > 0 || (cfg.is_priority_kick(**id) && ec.started + ec.success > 0)
            })
            .collect();
        cast_ids.sort_by_key(|(id, _)| **id);
        let mut enemy_casts: Vec<EnemyCastReport> = vec![];
        for (id, ec) in cast_ids {
            match enemy_casts.iter_mut().find(|c| c.name == ec.name) {
                Some(c) => {
                    c.started += ec.started;
                    c.succeeded += ec.success;
                    c.interrupted += ec.interrupted;
                    c.priority |= cfg.is_priority_kick(*id);
                }
                None => enemy_casts.push(EnemyCastReport {
                    spell_id: *id,
                    name: ec.name.clone(),
                    started: ec.started,
                    succeeded: ec.success,
                    interrupted: ec.interrupted,
                    priority: cfg.is_priority_kick(*id),
                }),
            }
        }
        enemy_casts.sort_by(|a, b| {
            b.succeeded
                .cmp(&a.succeeded)
                .then_with(|| a.name.cmp(&b.name))
        });

        let pulls: Vec<PullReport> = self
            .pulls
            .iter()
            .enumerate()
            .map(|(i, p)| PullReport {
                index: i + 1,
                start_s: secs(p.start - start),
                end_s: secs(p.end - start),
                deaths: p.deaths,
                enemies_killed: p.kills,
                boss: p.boss.clone(),
            })
            .collect();
        let mut deaths = std::mem::take(&mut self.deaths);
        for d in deaths.iter_mut() {
            d.pull = d.pull.map(|i| i + 1);
        }
        // On a raid wipe, once ~a quarter of the raid is dead the pull is lost;
        // later deaths are cascade noise and shouldn't drive suggestions.
        if matches!(self.seed, Seed::Raid { .. }) && end.success != Some(true) {
            let k = (players.len() / 4).max(3);
            for d in deaths.iter_mut().skip(k) {
                d.wipe_cascade = true;
            }
        }

        let kind = match self.seed {
            Seed::Raid {
                id,
                name,
                difficulty,
                size,
            } => {
                let success = end.success.unwrap_or(false);
                let hp = if success {
                    Some(0.0)
                } else {
                    self.boss_track
                        .as_ref()
                        .filter(|b| b.2 > 0)
                        .map(|b| 100.0 * b.1 as f64 / b.2 as f64)
                };
                FightKind::Raid {
                    encounter_id: id,
                    name,
                    difficulty: difficulty_name(difficulty),
                    group_size: size,
                    success,
                    boss_hp_pct: hp,
                }
            }
            Seed::MPlus {
                zone,
                map_id,
                level,
                affixes,
            } => FightKind::MythicPlus {
                zone,
                map_id,
                level,
                affixes,
                success: end.success.unwrap_or(false),
                time_s: end.time_ms.map(secs).unwrap_or(dur),
                bosses: self.bosses.clone(),
            },
        };
        let title = match &kind {
            FightKind::Raid {
                name,
                difficulty,
                success,
                boss_hp_pct,
                ..
            } => format!(
                "{difficulty} {name} — {}",
                if *success {
                    "Kill".to_string()
                } else {
                    format!("Wipe ({:.1}%)", boss_hp_pct.unwrap_or(100.0))
                }
            ),
            FightKind::MythicPlus {
                zone,
                level,
                success,
                ..
            } => {
                format!(
                    "+{level} {zone} — {}",
                    if *success {
                        "Timed"
                    } else {
                        "Depleted / abandoned"
                    }
                )
            }
        };
        let lust_at_s = self.lust.iter().map(|t| secs(t - start)).collect();
        let tanks: Vec<&str> = players
            .iter()
            .filter(|p| p.role == Role::Tank)
            .map(|p| p.name.as_str())
            .collect();
        let mut spell_evidence: Vec<SpellEvidence> = self
            .enemy_extra
            .iter()
            .map(|(id, x)| {
                let ec = self.enemy_casts.get(id);
                let dmg = enemy_damage.iter().find(|d| d.spell_id == *id);
                let mut ctx: Vec<(&Option<String>, &u32)> = x.ctx.iter().collect();
                ctx.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                SpellEvidence {
                    spell_id: *id,
                    name: x.name.clone(),
                    boss: ctx.first().and_then(|c| c.0.clone()),
                    casts_started: ec.map_or(0, |c| c.started),
                    casts_succeeded: ec.map_or(0, |c| c.success),
                    interrupted: ec.map_or(0, |c| c.interrupted),
                    instant_casts: x.instant,
                    hits: dmg.map_or(0, |d| d.hits),
                    damage: dmg.map_or(0, |d| d.total),
                    players_hit: dmg.map_or(0, |d| d.players_hit),
                    tank_damage: dmg.map_or(0, |d| {
                        d.per_player
                            .iter()
                            .filter(|p| tanks.contains(&p.0.as_str()))
                            .map(|p| p.1)
                            .sum()
                    }),
                    non_tank_hit_frac: dmg.map_or(0.0, |d| d.non_tank_hit_frac),
                    likely_avoidable: dmg.is_some_and(|d| d.likely_avoidable),
                    applied: x.applied,
                    dispelled: x.dispelled,
                }
            })
            .collect();
        spell_evidence.sort_by_key(|e| e.spell_id);
        FightReport {
            start_ts: start,
            title,
            kind,
            duration_s: dur,
            complete: end.complete,
            players,
            deaths,
            pulls,
            downtime_s,
            enemy_casts,
            enemy_damage,
            lust_at_s,
            suggestions: vec![],
            spell_evidence,
        }
    }
}

struct EndInfo {
    success: Option<bool>,
    time_ms: Option<i64>,
    complete: bool,
}

// --------------------------------------------------------------- analyzer

pub struct Analyzer {
    pub cfg: Config,
    specs: HashMap<String, u32>,
    owners: HashMap<String, String>,
    cur: Option<FightBuilder>,
    pub fights: Vec<FightReport>,
    last_ts: i64,
}

impl Analyzer {
    pub fn new(cfg: Config) -> Self {
        Analyzer {
            cfg,
            specs: HashMap::new(),
            owners: HashMap::new(),
            cur: None,
            fights: vec![],
            last_ts: 0,
        }
    }

    fn close(&mut self, ts: i64, end: EndInfo) {
        if let Some(b) = self.cur.take() {
            let r = b.finish(ts, end, &self.cfg, &self.specs);
            if r.duration_s >= 5.0 && !r.players.is_empty() {
                self.fights.push(r);
            }
        }
    }

    pub fn feed(&mut self, e: Event) {
        self.last_ts = e.ts;
        let ts = e.ts;
        match &e.kind {
            Kind::Combatant { guid, spec_id } => {
                self.specs.insert(guid.clone(), *spec_id);
            }
            Kind::Summon { .. } => {
                if let (Some(s), Some(d)) = (&e.src, &e.dst)
                    && !s.is_hostile()
                {
                    self.owners.insert(d.guid.clone(), s.guid.clone());
                }
            }
            Kind::ChallengeStart {
                zone,
                map_id,
                level,
                affixes,
            } => {
                self.close(
                    ts,
                    EndInfo {
                        success: None,
                        time_ms: None,
                        complete: false,
                    },
                );
                let seed = Seed::MPlus {
                    zone: zone.clone(),
                    map_id: *map_id,
                    level: *level,
                    affixes: affixes.clone(),
                };
                self.cur = Some(FightBuilder::new(seed, ts));
                return;
            }
            Kind::ZoneChange { instance_id } => {
                // Leaving the instance ends any open run/encounter, otherwise an
                // abandoned key would swallow whatever the player does next.
                let leave = match self.cur.as_ref().map(|c| &c.seed) {
                    Some(Seed::MPlus { map_id, .. }) => map_id != instance_id,
                    Some(Seed::Raid { .. }) => true,
                    None => false,
                };
                if leave {
                    let last = self.cur.as_ref().map(|c| c.last_ts).unwrap_or(ts);
                    self.close(
                        last,
                        EndInfo {
                            success: None,
                            time_ms: None,
                            complete: false,
                        },
                    );
                }
                return;
            }
            Kind::ChallengeEnd {
                success, time_ms, ..
            } => {
                if matches!(self.cur.as_ref().map(|c| &c.seed), Some(Seed::MPlus { .. })) {
                    let t = (*time_ms > 0).then_some(*time_ms);
                    self.close(
                        ts,
                        EndInfo {
                            success: Some(*success),
                            time_ms: t,
                            complete: true,
                        },
                    );
                }
                return;
            }
            Kind::EncounterStart {
                id,
                name,
                difficulty,
                group_size,
            } => {
                if let Some(c) = &mut self.cur
                    && matches!(c.seed, Seed::MPlus { .. })
                {
                    c.open_boss = Some((name.clone(), ts, *id));
                    return;
                }
                self.close(
                    ts,
                    EndInfo {
                        success: None,
                        time_ms: None,
                        complete: false,
                    },
                );
                let seed = Seed::Raid {
                    id: *id,
                    name: name.clone(),
                    difficulty: *difficulty,
                    size: *group_size,
                };
                self.cur = Some(FightBuilder::new(seed, ts));
                return;
            }
            Kind::EncounterEnd { success, .. } => {
                if let Some(c) = &mut self.cur {
                    if matches!(c.seed, Seed::MPlus { .. }) {
                        if let Some((n, st, enc)) = c.open_boss.take() {
                            let start = c.start;
                            c.bosses.push(BossMarker {
                                name: n,
                                encounter_id: enc,
                                start_s: secs(st - start),
                                end_s: secs(ts - start),
                                success: *success,
                            });
                        }
                    } else {
                        self.close(
                            ts,
                            EndInfo {
                                success: Some(*success),
                                time_ms: None,
                                complete: true,
                            },
                        );
                    }
                }
                return;
            }
            _ => {}
        }
        // learn pet ownership from advanced info
        let info = match &e.kind {
            Kind::Damage { info, .. }
            | Kind::Heal { info, .. }
            | Kind::CastSuccess { info, .. } => info.as_ref(),
            _ => None,
        };
        if let Some(i) = info
            && i.owner_guid != "0000000000000000"
            && i.owner_guid != i.guid
            && !i.owner_guid.is_empty()
        {
            self.owners
                .entry(i.guid.clone())
                .or_insert_with(|| i.owner_guid.clone());
        }
        if let Some(c) = &mut self.cur {
            c.ingest(&e, &self.owners, &self.cfg);
        }
    }

    /// Close any open fight (log ended mid-pull) and return all fights.
    pub fn finish(mut self) -> (Vec<FightReport>, Config) {
        let ts = self.last_ts;
        self.close(
            ts,
            EndInfo {
                success: None,
                time_ms: None,
                complete: false,
            },
        );
        (self.fights, self.cfg)
    }
}

/// Aggregate repeated raid pulls of the same boss/difficulty.
pub fn progression(fights: &[FightReport]) -> Vec<ProgressionReport> {
    let mut groups: Vec<((u32, String), Vec<&FightReport>)> = vec![];
    for f in fights {
        if let FightKind::Raid {
            encounter_id,
            difficulty,
            ..
        } = &f.kind
        {
            let key = (*encounter_id, difficulty.clone());
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => g.1.push(f),
                None => groups.push((key, vec![f])),
            }
        }
    }
    let count = |v: Vec<String>| -> Vec<(String, u32)> {
        let mut m: HashMap<String, u32> = HashMap::new();
        for s in v {
            *m.entry(s).or_default() += 1;
        }
        let mut out: Vec<_> = m.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out.truncate(6);
        out
    };
    groups
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|((id, diff), v)| {
            let name = match &v[0].kind {
                FightKind::Raid { name, .. } => name.clone(),
                _ => unreachable!(),
            };
            let kills = v
                .iter()
                .filter(|f| matches!(f.kind, FightKind::Raid { success: true, .. }))
                .count() as u32;
            let best = v
                .iter()
                .filter_map(|f| match f.kind {
                    FightKind::Raid { boss_hp_pct, .. } => boss_hp_pct,
                    _ => None,
                })
                .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.min(b))));
            ProgressionReport {
                encounter_id: id,
                name,
                difficulty: diff,
                attempts: v.len() as u32,
                kills,
                best_hp_pct: best,
                avg_duration_s: v.iter().map(|f| f.duration_s).sum::<f64>() / v.len() as f64,
                death_causes: count(
                    v.iter()
                        .flat_map(|f| {
                            f.deaths
                                .iter()
                                .filter(|d| !d.wipe_cascade)
                                .map(|d| strip_src(&d.killing_blow))
                        })
                        .collect(),
                ),
                first_death_causes: count(
                    v.iter()
                        .filter_map(|f| f.deaths.first())
                        .map(|d| strip_src(&d.killing_blow))
                        .collect(),
                ),
                frequent_deaths: count(
                    v.iter()
                        .flat_map(|f| {
                            // one count per pull the player died in (pre-cascade)
                            let mut who: Vec<String> = f
                                .deaths
                                .iter()
                                .filter(|d| !d.wipe_cascade)
                                .map(|d| d.player.clone())
                                .collect();
                            who.sort();
                            who.dedup();
                            who
                        })
                        .collect(),
                ),
                suggestions: vec![],
            }
        })
        .collect()
}

fn strip_src(kb: &str) -> String {
    kb.split(" (").next().unwrap_or(kb).to_string()
}

#[cfg(test)]
mod tests;
