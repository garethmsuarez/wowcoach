//! Class abilities that coaching rules reason about: interrupts, crowd
//! control that stops casts, defensives and dispels (`data/classes.toml`).
//!
//! This is expansion data, curated by hand and checked three ways by
//! `wowcoach classes check`: internal consistency ([`ClassData::problems`]),
//! the Blizzard spell API (`--live`), and real logs ([`Audit`]: every
//! interrupt players used must be listed; nothing may be reused faster than
//! its listed cooldown allows).

use crate::model::{Event, Kind, Pos, Unit};
use crate::specs::{SPECS, class_specs, spec_info};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const EMBEDDED: &str = include_str!("../../../data/classes.toml");

/// `class` value for racial abilities.
pub const RACIAL: &str = "Racial";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbilityKind {
    Interrupt,
    /// Crowd control that stops a cast (see `cc`).
    Stop,
    Defensive,
    Dispel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cc {
    Stun,
    Incapacitate,
    Disorient,
    Fear,
    Knock,
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Personal,
    External,
    Group,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispelType {
    Magic,
    Curse,
    Poison,
    Disease,
    Bleed,
    Enrage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Friendly,
    Enemy,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Ability {
    /// The cast and any separate effect IDs it's logged under.
    pub ids: Vec<u32>,
    pub name: String,
    /// Class name as in `specs.rs`, or "Racial".
    pub class: String,
    /// Spec names; empty = every spec of the class.
    #[serde(default)]
    pub specs: Vec<String>,
    pub kind: AbilityKind,
    /// Base cooldown in seconds (talents may shorten it).
    pub cd_s: f64,
    #[serde(default)]
    pub cc: Option<Cc>,
    #[serde(default)]
    pub scope: Option<Scope>,
    #[serde(default)]
    pub types: Vec<DispelType>,
    #[serde(default)]
    pub target: Option<Target>,
    /// Talents routinely cut the cooldown a lot, so observed reuse says
    /// nothing about `cd_s`; the log audit skips it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cdr: bool,
    /// Reach in yards (interrupts: required; stops: optional). Melee is 5.
    #[serde(default)]
    pub range_yd: Option<f32>,
    /// Stops: the aura(s) the CC is logged as on the target, when they differ
    /// from `ids` (Shockwave is cast as 46968 but stuns as 132168).
    #[serde(default)]
    pub aura_ids: Vec<u32>,
    /// Stops: credited when it lands, but never suggested as an available
    /// stop. For damage cooldowns that happen to stun (Havoc Metamorphosis'
    /// landing) and stuns with a condition the log can't see (Rake from
    /// stealth).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub situational: bool,
}

impl Ability {
    /// Aura IDs that mean this stop landed: `aura_ids`, or the cast IDs.
    pub fn stop_auras(&self) -> &[u32] {
        if self.aura_ids.is_empty() {
            &self.ids
        } else {
            &self.aura_ids
        }
    }

    /// Can a player of this class and spec have it?
    pub fn available_to(&self, class: &str, spec: &str) -> bool {
        self.class == RACIAL
            || (self.class == class
                && (self.specs.is_empty() || self.specs.iter().any(|s| s == spec)))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClassData {
    #[serde(default, rename = "ability")]
    pub abilities: Vec<Ability>,
}

impl ClassData {
    pub fn parse(text: &str) -> anyhow::Result<ClassData> {
        let d: ClassData = toml::from_str(text)?;
        let p = d.problems();
        if !p.is_empty() {
            anyhow::bail!("invalid class table:\n  - {}", p.join("\n  - "));
        }
        Ok(d)
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<ClassData> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// The embedded table, parsed once and shared (analysis uses it per event).
    pub fn shared() -> &'static ClassData {
        static DATA: std::sync::OnceLock<ClassData> = std::sync::OnceLock::new();
        DATA.get_or_init(ClassData::embedded)
    }

    /// Interrupts a spec has. The first non-racial one is its main interrupt,
    /// which everyone of that spec can be assumed to have; the others
    /// (talents, racials) only count once a player actually uses them.
    pub fn interrupts_for(&self, spec_id: u32) -> (Option<&Ability>, Vec<&Ability>) {
        let all = self.for_spec(spec_id, AbilityKind::Interrupt);
        let main = all.iter().copied().find(|a| a.class != RACIAL);
        let extra = all
            .into_iter()
            .filter(|a| main.is_none_or(|m| !std::ptr::eq(*a, m)))
            .collect();
        (main, extra)
    }

    pub fn embedded() -> ClassData {
        Self::parse(EMBEDDED).unwrap_or_else(|e| panic!("embedded class table is invalid: {e}"))
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        let mut seen: HashMap<(u32, AbilityKind), &str> = HashMap::new();
        for a in &self.abilities {
            let label = format!("{} ({}, {:?})", a.name, a.class, a.kind);
            if a.class != RACIAL {
                let specs = class_specs(&a.class);
                if specs.is_empty() {
                    out.push(format!("{label}: unknown class"));
                }
                for s in &a.specs {
                    if !specs.contains(&s.as_str()) {
                        out.push(format!("{label}: {s:?} is not a {} spec", a.class));
                    }
                }
            } else if !a.specs.is_empty() {
                out.push(format!("{label}: racials have no specs"));
            }
            if a.ids.is_empty() || a.ids.contains(&0) {
                out.push(format!("{label}: needs non-zero ids"));
            }
            for id in &a.ids {
                if let Some(other) = seen.insert((*id, a.kind), &a.name) {
                    out.push(format!(
                        "{label}: id {id} is also listed for {other:?} with the same kind"
                    ));
                }
            }
            if a.cd_s.is_nan() || a.cd_s <= 0.0 {
                out.push(format!("{label}: cd_s must be positive"));
            }
            let (cc, scope, dispel) = (
                a.cc.is_some(),
                a.scope.is_some(),
                !a.types.is_empty() || a.target.is_some(),
            );
            match a.kind {
                AbilityKind::Stop if !cc => out.push(format!("{label}: a stop needs `cc`")),
                AbilityKind::Defensive if !scope => {
                    out.push(format!("{label}: a defensive needs `scope`"))
                }
                AbilityKind::Dispel if a.types.is_empty() || a.target.is_none() => {
                    out.push(format!("{label}: a dispel needs `types` and `target`"))
                }
                _ => {}
            }
            match (a.kind, a.range_yd) {
                (AbilityKind::Interrupt, None) => {
                    out.push(format!("{label}: an interrupt needs `range_yd`"))
                }
                (AbilityKind::Interrupt | AbilityKind::Stop, Some(r))
                    if !(1.0..=100.0).contains(&r) =>
                {
                    out.push(format!("{label}: range_yd {r} is out of 1..100"))
                }
                (AbilityKind::Defensive | AbilityKind::Dispel, Some(_)) => {
                    out.push(format!("{label}: range_yd is for interrupts and stops"))
                }
                _ => {}
            }
            if !a.aura_ids.is_empty() && a.kind != AbilityKind::Stop {
                out.push(format!("{label}: aura_ids is for stops"));
            }
            if a.situational && a.kind != AbilityKind::Stop {
                out.push(format!("{label}: situational is for stops"));
            }
            if (cc && a.kind != AbilityKind::Stop)
                || (scope && a.kind != AbilityKind::Defensive)
                || (dispel && a.kind != AbilityKind::Dispel)
            {
                out.push(format!("{label}: has fields for another kind"));
            }
        }
        out
    }

    /// Abilities of `kind` a spec has (including racials, which depend on race).
    pub fn for_spec(&self, spec_id: u32, kind: AbilityKind) -> Vec<&Ability> {
        let Some((class, spec, _)) = spec_info(spec_id) else {
            return vec![];
        };
        self.abilities
            .iter()
            .filter(|a| a.kind == kind && a.available_to(class, spec))
            .collect()
    }

    /// The stop whose aura this is, if any.
    pub fn stop_by_aura(&self, aura_id: u32) -> Option<&Ability> {
        self.abilities
            .iter()
            .find(|a| a.kind == AbilityKind::Stop && a.stop_auras().contains(&aura_id))
    }

    /// Every entry listing this spell ID (one ability can be e.g. both a stop and an interrupt).
    pub fn by_id(&self, id: u32) -> Vec<&Ability> {
        self.abilities
            .iter()
            .filter(|a| a.ids.contains(&id))
            .collect()
    }

    pub fn summary(&self) -> String {
        let mut by_kind: BTreeMap<AbilityKind, usize> = BTreeMap::new();
        for a in &self.abilities {
            *by_kind.entry(a.kind).or_default() += 1;
        }
        let classes: BTreeSet<&str> = self
            .abilities
            .iter()
            .map(|a| a.class.as_str())
            .filter(|c| *c != RACIAL)
            .collect();
        let no_interrupt: Vec<String> = SPECS
            .iter()
            .filter(|s| {
                self.for_spec(s.0, AbilityKind::Interrupt)
                    .iter()
                    .all(|a| a.class == RACIAL)
            })
            .map(|s| format!("{} {}", s.2, s.1))
            .collect();
        format!(
            "{} abilities for {} classes: {} interrupts, {} stops, {} defensives, {} dispels. Specs without a class interrupt: {}.",
            self.abilities.len(),
            classes.len(),
            by_kind.get(&AbilityKind::Interrupt).unwrap_or(&0),
            by_kind.get(&AbilityKind::Stop).unwrap_or(&0),
            by_kind.get(&AbilityKind::Defensive).unwrap_or(&0),
            by_kind.get(&AbilityKind::Dispel).unwrap_or(&0),
            if no_interrupt.is_empty() {
                "none".into()
            } else {
                no_interrupt.join(", ")
            }
        )
    }
}

// --------------------------------------------------------------- log audit

/// Checks a class table against what players actually did in combat logs.
#[derive(Default)]
pub struct Audit {
    specs: HashMap<String, u32>,
    owners: HashMap<String, String>,
    /// (player guid, spell id) -> cast timestamps.
    casts: HashMap<(String, u32), Vec<i64>>,
    /// (class, spec, spell id, name) -> uses, for interrupt/dispel events.
    interrupts: BTreeMap<(String, String, u32, String), u32>,
    dispels: BTreeMap<(String, String, u32, String), u32>,
    /// guid -> (class, spec) of players seen acting.
    players: HashMap<String, (String, String)>,
    /// Units created by SPELL_SUMMON (totems, guardians): their auras count as
    /// the group's. Advanced-logging ownership also covers NPC allies such as
    /// delve companions, so it isn't used for this.
    summoned: BTreeSet<String>,
    /// Last known position of every unit with advanced info.
    pos: HashMap<String, Pos>,
    /// Interrupt spell id -> kicker-to-caster distances (yards).
    kick_dist: HashMap<u32, Vec<f32>>,
    /// Inside a Mythic+ key: cast outcomes are only collected there (delves
    /// have NPC companions whose CC would be miscounted as the group's).
    in_key: bool,
    /// Hostile caster guid -> (spell, cast start) while its cast bar runs.
    casting: HashMap<String, (u32, i64)>,
    /// Hostile unit guid -> recent harmful auras from the group: (ts, id, name).
    debuffs: HashMap<String, Vec<(i64, u32, String)>>,
    /// Cast bars that ended silently / completed, and which group auras
    /// landed during them.
    silent: u32,
    completed: u32,
    aura_in_silent: HashMap<(u32, String), u32>,
    aura_in_completed: HashMap<u32, u32>,
}

/// Findings, most important first.
#[derive(Debug, Default, PartialEq)]
pub struct AuditReport {
    /// Interrupts players used that the table doesn't list for their class/spec.
    pub missing_interrupts: Vec<String>,
    /// Dispels not in the table (often side effects of other abilities).
    pub unlisted_dispels: Vec<String>,
    /// Usually reused faster than the listed cooldown: likely a wrong cooldown.
    pub fast_reuse: Vec<String>,
    /// Abilities of classes/specs present in the logs that were never used.
    pub unseen: Vec<String>,
    /// Interrupts usually used from further away than `range_yd` allows.
    pub kick_ranges: Vec<String>,
    /// Kick distances per interrupt that had enough samples, for reference.
    pub kick_distance_summary: Vec<String>,
    /// Group auras that land on casters whose cast then ends silently, but
    /// rarely during casts that complete: likely stops missing from the table.
    pub stop_candidates: Vec<String>,
    /// Silently-ended cast bars, and how many a listed stop explains.
    pub silent_casts: u32,
    pub silent_explained: u32,
    pub players: usize,
}

/// A stop lands during at least this many silently-ended casts...
const STOP_MIN: u32 = 5;
/// ...and this many times more often than during completed casts. Real stops
/// in Midnight logs scored 100-1300x; damage and slows that merely correlate, 5-30x.
const STOP_LIFT: f64 = 50.0;

impl Audit {
    fn player(&self, u: &Option<Unit>) -> Option<(String, (String, String))> {
        let u = u.as_ref()?;
        let guid = if u.is_player() {
            u.guid.clone()
        } else if u.guid.starts_with("Pet-") {
            // Class pets (a warlock's Felhunter) act for their owner. Guardians
            // and companions (`Creature-`, e.g. delve companions) don't.
            self.owners.get(&u.guid)?.clone()
        } else {
            return None;
        };
        let (class, spec, _) = spec_info(*self.specs.get(&guid)?)?;
        Some((guid, (class.to_string(), spec.to_string())))
    }

    /// A hostile caster's cast bar ended without completing, being kicked or
    /// the caster dying: note which group auras landed during it.
    fn end_cast(&mut self, caster: &str, start: i64, end: i64, completed: bool) {
        let seen: BTreeSet<(u32, String)> = self
            .debuffs
            .get(caster)
            .into_iter()
            .flatten()
            .filter(|(t, _, _)| *t >= start - 50 && *t <= end)
            .map(|(_, id, n)| (*id, n.clone()))
            .collect();
        if completed {
            self.completed += 1;
            for (id, _) in seen {
                *self.aura_in_completed.entry(id).or_default() += 1;
            }
        } else {
            self.silent += 1;
            for k in seen {
                *self.aura_in_silent.entry(k).or_default() += 1;
            }
        }
    }

    pub fn feed(&mut self, e: &Event) {
        let info = match &e.kind {
            Kind::Damage { info, .. }
            | Kind::Heal { info, .. }
            | Kind::CastSuccess { info, .. } => info.as_ref(),
            _ => None,
        };
        if let Some(i) = info
            && let Some(p) = i.pos
        {
            self.pos.insert(i.guid.clone(), p);
        }
        match &e.kind {
            Kind::ChallengeStart { .. } => self.in_key = true,
            Kind::ChallengeEnd { .. } => {
                self.in_key = false;
                self.casting.clear();
                self.debuffs.clear();
            }
            _ => {}
        }
        let hostile_src = e.src.as_ref().filter(|u| self.in_key && u.is_hostile_npc());
        match &e.kind {
            Kind::CastStart { spell } => {
                if let Some(c) = hostile_src {
                    if let Some((_, st)) = self.casting.remove(&c.guid) {
                        self.end_cast(&c.guid.clone(), st, e.ts, false);
                    }
                    self.casting.insert(c.guid.clone(), (spell.id, e.ts));
                }
            }
            Kind::CastSuccess { spell, .. } => {
                if let Some(c) = hostile_src
                    && self
                        .casting
                        .get(&c.guid)
                        .is_some_and(|(id, _)| *id == spell.id)
                {
                    let (_, st) = self.casting.remove(&c.guid).unwrap();
                    self.end_cast(&c.guid.clone(), st, e.ts, true);
                }
            }
            Kind::Death => {
                if let Some(d) = &e.dst {
                    self.casting.remove(&d.guid);
                    self.debuffs.remove(&d.guid);
                }
            }
            Kind::AuraApplied {
                spell,
                is_buff: false,
            } => {
                // From players, their pets and things they summoned (totems), not
                // NPC allies such as delve companions.
                if let (Some(s), Some(d)) = (&e.src, &e.dst)
                    && self.in_key
                    && !s.is_hostile()
                    && (s.is_player()
                        || s.guid.starts_with("Pet-")
                        || self.summoned.contains(&s.guid))
                    && d.is_hostile_npc()
                {
                    let v = self.debuffs.entry(d.guid.clone()).or_default();
                    v.retain(|(t, _, _)| e.ts - t < 30_000);
                    v.push((e.ts, spell.id, spell.name.clone()));
                }
            }
            _ => {}
        }
        match &e.kind {
            Kind::Combatant { guid, spec_id } => {
                self.specs.insert(guid.clone(), *spec_id);
            }
            Kind::Summon { .. } => {
                if let (Some(s), Some(d)) = (&e.src, &e.dst) {
                    self.owners.insert(d.guid.clone(), s.guid.clone());
                    self.summoned.insert(d.guid.clone());
                }
            }
            Kind::CastSuccess { spell, info } => {
                if let Some(i) = info
                    && !i.owner_guid.is_empty()
                    && i.owner_guid != "0000000000000000"
                    && i.owner_guid != i.guid
                {
                    self.owners
                        .entry(i.guid.clone())
                        .or_insert_with(|| i.owner_guid.clone());
                }
                if let Some((guid, cs)) = self.player(&e.src) {
                    self.casts
                        .entry((guid.clone(), spell.id))
                        .or_default()
                        .push(e.ts);
                    self.players.insert(guid, cs);
                }
            }
            Kind::Interrupt { spell, .. } => {
                if let Some(d) = &e.dst {
                    self.casting.remove(&d.guid);
                    if let (Some(k), Some(t)) = (
                        e.src.as_ref().and_then(|u| self.pos.get(&u.guid)),
                        self.pos.get(&d.guid),
                    ) && let Some(dist) = k.distance(t)
                    {
                        self.kick_dist.entry(spell.id).or_default().push(dist);
                    }
                }
                if let Some((guid, (c, s))) = self.player(&e.src) {
                    *self
                        .interrupts
                        .entry((c.clone(), s.clone(), spell.id, spell.name.clone()))
                        .or_default() += 1;
                    self.players.insert(guid, (c, s));
                }
            }
            Kind::Dispel { spell, .. } => {
                if let Some((guid, (c, s))) = self.player(&e.src) {
                    *self
                        .dispels
                        .entry((c.clone(), s.clone(), spell.id, spell.name.clone()))
                        .or_default() += 1;
                    self.players.insert(guid, (c, s));
                }
            }
            _ => {}
        }
    }

    pub fn report(&self, data: &ClassData) -> AuditReport {
        let mut r = AuditReport {
            players: self.players.len(),
            ..Default::default()
        };
        let listed = |id: u32, kind: AbilityKind, c: &str, s: &str| {
            data.abilities
                .iter()
                .any(|a| a.kind == kind && a.ids.contains(&id) && a.available_to(c, s))
        };
        for ((c, s, id, name), n) in &self.interrupts {
            if !listed(*id, AbilityKind::Interrupt, c, s) {
                r.missing_interrupts
                    .push(format!("{name} ({id}) used {n}x by {s} {c}"));
            }
        }
        for ((c, s, id, name), n) in &self.dispels {
            if !listed(*id, AbilityKind::Dispel, c, s) {
                r.unlisted_dispels
                    .push(format!("{name} ({id}) dispelled {n}x for {s} {c}"));
            }
        }

        let present: BTreeSet<(&str, &str)> = self
            .players
            .values()
            .map(|(c, s)| (c.as_str(), s.as_str()))
            .collect();
        let used_ids: BTreeSet<u32> = self
            .casts
            .keys()
            .map(|k| k.1)
            .chain(self.interrupts.keys().map(|k| k.2))
            .chain(self.dispels.keys().map(|k| k.2))
            .collect();
        for a in &data.abilities {
            if a.class == RACIAL {
                continue;
            }
            let specs: Vec<&str> = present
                .iter()
                .filter(|(c, s)| a.available_to(c, s))
                .map(|(_, s)| *s)
                .collect();
            if !specs.is_empty() && !a.ids.iter().any(|id| used_ids.contains(id)) {
                r.unseen
                    .push(format!("{} ({}: {})", a.name, a.class, specs.join(", ")));
            }
            // Reuse gaps per player. One fast reuse proves little (charges,
            // procs, duplicate events); a wrong cooldown shows as *usually* fast.
            // Only players of the listed classes/specs count (another spec may
            // cast the same ID differently), and dispels are skipped: their
            // cooldown only starts when something is actually removed.
            let mut gaps: Vec<f64> = vec![];
            for ((guid, id), times) in &self.casts {
                let eligible = self
                    .players
                    .get(guid)
                    .is_some_and(|(c, s)| a.available_to(c, s));
                if a.kind == AbilityKind::Dispel || a.cdr || !eligible || !a.ids.contains(id) {
                    continue;
                }
                let mut t = times.clone();
                t.sort_unstable();
                gaps.extend(
                    t.windows(2)
                        .map(|w| (w[1] - w[0]) as f64 / 1000.0)
                        .filter(|g| *g > 0.5),
                );
            }
            let fast = gaps.iter().filter(|g| **g < 0.6 * a.cd_s).count();
            if gaps.len() >= 3 && fast * 2 >= gaps.len() {
                gaps.sort_by(f64::total_cmp);
                r.fast_reuse.push(format!(
                    "{} ({}): {fast} of {} reuses under 60% of the listed {}s, median {:.1}s",
                    a.name,
                    a.class,
                    gaps.len(),
                    a.cd_s,
                    gaps[gaps.len() / 2]
                ));
            }
        }
        // Kick distances against listed ranges. Positions are the last ones
        // logged, and units have hitboxes, so allow a few yards.
        let mut kick_ids: Vec<&u32> = self.kick_dist.keys().collect();
        kick_ids.sort();
        for id in kick_ids {
            let mut d = self.kick_dist[id].clone();
            if d.len() < 5 {
                continue;
            }
            d.sort_by(f32::total_cmp);
            let (med, p90) = (d[d.len() / 2], d[d.len() * 9 / 10]);
            let Some(a) = data
                .abilities
                .iter()
                .find(|a| a.kind == AbilityKind::Interrupt && a.ids.contains(id))
            else {
                continue;
            };
            let line = format!(
                "{} ({id}): {} kicks, median {med:.1} yd, 90th percentile {p90:.1} yd",
                a.name,
                d.len()
            );
            if let Some(range) = a.range_yd
                && med > range + 5.0
            {
                r.kick_ranges
                    .push(format!("{line}; listed range {range} yd"));
            }
            r.kick_distance_summary.push(line);
        }

        // Stops: auras that land during silent cast endings far more than
        // during completed casts.
        r.silent_casts = self.silent;
        let silent = self.silent.max(1) as f64;
        let completed = self.completed.max(1) as f64;
        let mut explained_ids: BTreeSet<u32> = BTreeSet::new();
        let mut cands: Vec<(f64, String)> = vec![];
        for ((id, name), n) in &self.aura_in_silent {
            let in_completed = *self.aura_in_completed.get(id).unwrap_or(&0) as f64;
            let lift = (*n as f64 / silent) / ((in_completed + 0.5) / completed);
            if data.stop_by_aura(*id).is_some() {
                explained_ids.insert(*id);
            } else if *n >= STOP_MIN && lift >= STOP_LIFT {
                cands.push((lift, format!("{name} ({id}): during {n} silent endings, {in_completed:.0} completed casts (lift {lift:.0}x)")));
            }
        }
        cands.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        r.stop_candidates = cands.into_iter().map(|c| c.1).collect();
        r.silent_explained = self
            .aura_in_silent
            .keys()
            .filter(|(id, _)| explained_ids.contains(id))
            .map(|k| self.aura_in_silent[k])
            .sum::<u32>()
            .min(self.silent);
        r
    }
}

impl std::fmt::Display for AuditReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Checked against {} players in the logs.", self.players)?;
        if self.silent_casts > 0 {
            writeln!(
                f,
                "Enemy cast bars that ended silently (no completion, kick or death): {}; a listed stop landed during {} of them.",
                self.silent_casts, self.silent_explained
            )?;
        }
        let section =
            |f: &mut std::fmt::Formatter<'_>, title: &str, v: &[String]| -> std::fmt::Result {
                if !v.is_empty() {
                    writeln!(f, "{title} ({}):", v.len())?;
                    for x in v {
                        writeln!(f, "  {x}")?;
                    }
                }
                Ok(())
            };
        section(
            f,
            "ERROR interrupts used but not listed",
            &self.missing_interrupts,
        )?;
        section(
            f,
            "Dispels not listed (often side effects of other abilities)",
            &self.unlisted_dispels,
        )?;
        section(
            f,
            "Usually reused faster than the listed cooldown (check cd_s)",
            &self.fast_reuse,
        )?;
        section(
            f,
            "Interrupts used from further than their listed range (check range_yd)",
            &self.kick_ranges,
        )?;
        section(
            f,
            "Likely stops missing from the table (add the aura to a stop's aura_ids)",
            &self.stop_candidates,
        )?;
        section(
            f,
            "Never used by the classes/specs present (talents not taken?)",
            &self.unseen,
        )?;
        section(f, "Kick distances", &self.kick_distance_summary)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
