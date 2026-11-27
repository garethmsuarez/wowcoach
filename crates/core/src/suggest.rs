//! `wowcoach season suggest`: learn spell IDs, boss encounter IDs and tag
//! *proposals* from analysed fights (your own logs and/or top public keys on
//! Warcraft Logs), and merge them into a season pack.
//!
//! Tools never set `tags`. Proposals go to `suggested` with the counts behind
//! them in `evidence`; a reviewer promotes or dismisses them. Only spell IDs,
//! names and aggregate counts are kept: no player names.

use crate::analysis::{FightKind, FightReport, SpellEvidence};
use crate::classes::{AbilityKind, ClassData, DispelType, Target};
use crate::season::{Boss, SeasonPack, Source, SpellEntry, Tag, tag_name};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where a fight came from. Kept separate because "top groups kick this every
/// time" is stronger evidence than "we kicked it sometimes".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// The user's own logs or reports.
    Own,
    /// Top public runs on Warcraft Logs.
    Top,
}

impl Origin {
    fn source(self) -> Source {
        match self {
            Origin::Own => Source::Log,
            Origin::Top => Source::Wcl,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Origin::Own => "yours",
            Origin::Top => "top keys",
        }
    }
}

// Thresholds for proposals. Deliberately conservative: a reviewer sees the
// evidence either way, and a missing suggestion costs less than a wrong one.

/// A spell needs this many events (casts, hits, applications) to be added at all.
pub const MIN_EVENTS: u32 = 3;
/// No proposal from fewer fights than this: one key is an anecdote.
pub const MIN_FIGHTS: u32 = 2;
/// `priority_kick` / `kick`: kicked at least this often...
pub const MIN_KICKS: u32 = 3;
/// ...and `priority_kick` (must kick) when at least this share of
/// completed-or-kicked cast bars was kicked...
pub const PRIORITY_KICK_RATE: f64 = 0.9;
/// ...or `kick` (kick when free) from this share up to it. Top groups letting
/// one in ten through usually means they had something better to kick.
pub const KICK_RATE: f64 = 0.7;
/// `avoidable`: flagged by the uneven-hit heuristic in at least half of the
/// fights where it hit twice or more, in at least this many fights.
pub const MIN_AVOIDABLE_FIGHTS: u32 = 2;
/// `dispel_priority`: dispelled at least this often, and this share of applications.
pub const MIN_DISPELS: u32 = 3;
pub const DISPEL_RATE: f64 = 0.4;
/// `tank_buster`: this share of its damage lands on tanks, it has a cast bar,
/// and it hit at least `MIN_EVENTS` times.
pub const TANK_SHARE: f64 = 0.9;
/// `stoppable`: ended by a listed crowd-control stop at least this often.
/// The stops themselves are validated (lift >= 50x in the class audit), so a
/// few observations are enough to say CC works on the cast.
pub const MIN_STOPS: u32 = 3;
/// `dangerous_cast`: a top source of at least this many damage spikes...
pub const MIN_SPIKES: u32 = 3;
/// ...hitting several players per cast (a volley, not a bolt: a fight-wide
/// "who was ever hit" share can't tell those apart)...
pub const GROUP_HITS_PER_CAST: f64 = 2.5;
/// ...in a spike on at least this share of its casts (not a frequent cast that
/// merely lands during spikes), and not aimed at tanks.
pub const SPIKE_RATE: f64 = 0.2;
/// Affix mechanics: a spell seen only in keys with the affix, from units that
/// never appear in a key without it, with at least this many keys of the same
/// dungeon(s) without the affix to compare against, and no fewer than keys
/// with it.
pub const MIN_CONTROL_KEYS: usize = 2;
/// ...and it shows up in at least this share of that dungeon's keys with the
/// affix (an affix is in every key; a summon or an optional pack is not).
pub const AFFIX_PRESENCE: f64 = 0.5;

/// Counts for one spell from one origin, summed over fights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agg {
    pub fights: u32,
    pub casts_started: u32,
    pub casts_succeeded: u32,
    pub interrupted: u32,
    pub instant_casts: u32,
    pub hits: u32,
    pub damage: u64,
    pub tank_damage: u64,
    pub applied: u32,
    pub dispelled: u32,
    pub stopped: u32,
    pub spikes: u32,
    /// Fights where it hit at least twice.
    pub hit_fights: u32,
    /// Of those, fights the uneven-hit heuristic flagged.
    pub avoidable_fights: u32,
}

impl Agg {
    fn add(&mut self, e: &SpellEvidence) {
        self.fights += 1;
        self.casts_started += e.casts_started;
        self.casts_succeeded += e.casts_succeeded;
        self.interrupted += e.interrupted;
        self.instant_casts += e.instant_casts;
        self.hits += e.hits;
        self.damage += e.damage;
        self.tank_damage += e.tank_damage;
        self.applied += e.applied;
        self.dispelled += e.dispelled;
        self.stopped += e.stopped;
        self.spikes += e.spikes;
        if e.hits >= 2 {
            self.hit_fights += 1;
            if e.likely_avoidable {
                self.avoidable_fights += 1;
            }
        }
    }
    fn plus(&self, o: &Agg) -> Agg {
        Agg {
            fights: self.fights + o.fights,
            casts_started: self.casts_started + o.casts_started,
            casts_succeeded: self.casts_succeeded + o.casts_succeeded,
            interrupted: self.interrupted + o.interrupted,
            instant_casts: self.instant_casts + o.instant_casts,
            hits: self.hits + o.hits,
            damage: self.damage + o.damage,
            tank_damage: self.tank_damage + o.tank_damage,
            applied: self.applied + o.applied,
            dispelled: self.dispelled + o.dispelled,
            stopped: self.stopped + o.stopped,
            spikes: self.spikes + o.spikes,
            hit_fights: self.hit_fights + o.hit_fights,
            avoidable_fights: self.avoidable_fights + o.avoidable_fights,
        }
    }
    fn events(&self) -> u32 {
        self.casts_started + self.instant_casts + self.hits + self.applied + self.interrupted
    }
    fn kick_rate(&self) -> f64 {
        let n = self.interrupted + self.casts_succeeded;
        if n == 0 {
            0.0
        } else {
            self.interrupted as f64 / n as f64
        }
    }
    fn describe(&self) -> String {
        let mut parts = vec![];
        if self.interrupted + self.casts_succeeded > 0 {
            parts.push(format!(
                "kicked {}/{}",
                self.interrupted,
                self.interrupted + self.casts_succeeded
            ));
        }
        if self.instant_casts > 0 {
            parts.push(format!("{} instant", self.instant_casts));
        }
        if self.stopped > 0 {
            parts.push(format!("stopped by CC {}", self.stopped));
        }
        if self.spikes > 0 {
            parts.push(format!("in {} damage spikes", self.spikes));
        }
        if self.hits > 0 {
            let tank = if self.damage > 0 {
                100.0 * self.tank_damage as f64 / self.damage as f64
            } else {
                0.0
            };
            parts.push(format!("{} hits ({tank:.0}% on tanks)", self.hits));
        }
        if self.hit_fights > 0 && self.avoidable_fights > 0 {
            parts.push(format!(
                "uneven hits in {}/{} fights",
                self.avoidable_fights, self.hit_fights
            ));
        }
        if self.applied > 0 {
            parts.push(format!("dispelled {}/{}", self.dispelled, self.applied));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpellAgg {
    pub name: String,
    /// Pack boss name (None = trash) -> fights it was mostly seen in.
    pub boss_votes: BTreeMap<Option<String>, u32>,
    /// (fight number, origin, evidence) for every fight the spell was seen in.
    pub seen: Vec<(u32, Origin, SpellEvidence)>,
}

impl SpellAgg {
    /// Combine the evidence of several spell IDs that are one pack entry.
    pub fn merge(parts: &[&SpellAgg]) -> SpellAgg {
        let mut out = SpellAgg::default();
        for p in parts {
            if out.name.is_empty() {
                out.name.clone_from(&p.name);
            }
            for (b, n) in &p.boss_votes {
                *out.boss_votes.entry(b.clone()).or_default() += n;
            }
            out.seen.extend(p.seen.iter().cloned());
        }
        out
    }

    /// Counts per origin. IDs of one mechanic seen in the same fight are
    /// summed into that fight first, so a fight is counted once.
    pub fn by(&self) -> BTreeMap<Origin, Agg> {
        let mut fights: BTreeMap<(Origin, u32), SpellEvidence> = BTreeMap::new();
        for (n, o, e) in &self.seen {
            let f = fights.entry((*o, *n)).or_default();
            f.casts_started += e.casts_started;
            f.casts_succeeded += e.casts_succeeded;
            f.interrupted += e.interrupted;
            f.instant_casts += e.instant_casts;
            f.hits += e.hits;
            f.damage += e.damage;
            f.tank_damage += e.tank_damage;
            f.applied += e.applied;
            f.dispelled += e.dispelled;
            f.stopped += e.stopped;
            f.spikes += e.spikes;
            f.likely_avoidable |= e.likely_avoidable;
        }
        let mut out: BTreeMap<Origin, Agg> = BTreeMap::new();
        for ((o, _), e) in &fights {
            out.entry(*o).or_default().add(e);
        }
        out
    }

    pub fn total(&self) -> Agg {
        self.by().values().fold(Agg::default(), |a, b| a.plus(b))
    }

    /// The boss it belongs to; ties go to trash, then alphabetical.
    pub fn boss(&self) -> Option<String> {
        let best = self.boss_votes.values().copied().max().unwrap_or(0);
        self.boss_votes
            .iter()
            .find(|(_, n)| **n == best)
            .and_then(|(b, _)| b.clone())
    }

    /// Tags the evidence supports, before removing ones already tagged/dismissed.
    pub fn propose(&self) -> Vec<Tag> {
        let by = self.by();
        let all = by.values().fold(Agg::default(), |a, b| a.plus(b));
        let mut out = vec![];
        if all.fights < MIN_FIGHTS {
            return out;
        }
        // Kick evidence from top keys alone when there's enough of it: top
        // groups kicking something every time is the clearest signal.
        let kicks = match by.get(&Origin::Top) {
            Some(t) if t.interrupted + t.casts_succeeded >= MIN_KICKS => t.clone(),
            _ => all.clone(),
        };
        if kicks.interrupted >= MIN_KICKS && kicks.kick_rate() >= PRIORITY_KICK_RATE {
            out.push(Tag::PriorityKick);
        } else if kicks.interrupted >= MIN_KICKS && kicks.kick_rate() >= KICK_RATE {
            out.push(Tag::Kick);
        }
        let tank_share = if all.damage > 0 {
            all.tank_damage as f64 / all.damage as f64
        } else {
            0.0
        };
        if all.avoidable_fights >= MIN_AVOIDABLE_FIGHTS
            && all.avoidable_fights * 2 >= all.hit_fights
            && tank_share < 0.5
        {
            out.push(Tag::Avoidable);
        }
        if all.dispelled >= MIN_DISPELS && all.dispelled as f64 >= DISPEL_RATE * all.applied as f64
        {
            out.push(Tag::DispelPriority);
        }
        if all.hits >= MIN_EVENTS && all.casts_started > 0 && tank_share >= TANK_SHARE {
            out.push(Tag::TankBuster);
        }
        if all.stopped >= MIN_STOPS {
            out.push(Tag::Stoppable);
        }
        let landed = all.casts_succeeded + all.instant_casts;
        if all.spikes >= MIN_SPIKES
            && landed > 0
            && all.hits as f64 >= GROUP_HITS_PER_CAST * landed as f64
            && all.spikes as f64 >= SPIKE_RATE * landed as f64
            && tank_share < 0.5
        {
            out.push(Tag::DangerousCast);
        }
        out
    }

    /// The debuff types the dispels that removed it share: None when no
    /// listed friendly dispel removed it, empty when they share none.
    pub fn dispel_types(&self) -> Option<Vec<DispelType>> {
        let classes = ClassData::shared();
        let ids: BTreeSet<u32> = self
            .seen
            .iter()
            .flat_map(|(_, _, e)| e.dispelled_by.iter().copied())
            .collect();
        let mut common: Option<BTreeSet<DispelType>> = None;
        for id in ids {
            let Some(a) = classes
                .by_id(id)
                .into_iter()
                .find(|a| a.kind == AbilityKind::Dispel && a.target == Some(Target::Friendly))
            else {
                continue;
            };
            let t: BTreeSet<DispelType> = a.types.iter().copied().collect();
            common = Some(match common {
                None => t,
                Some(c) => c.intersection(&t).copied().collect(),
            });
        }
        common.map(|c| c.into_iter().collect())
    }

    /// NPC IDs of the units it came from, over all fights.
    pub fn npcs(&self) -> BTreeSet<u32> {
        self.seen
            .iter()
            .flat_map(|(_, _, e)| e.npcs.iter().copied())
            .collect()
    }

    pub fn evidence(&self) -> String {
        self.by()
            .iter()
            .map(|(o, a)| {
                format!(
                    "{} ({} fight{}): {}",
                    o.label(),
                    a.fights,
                    if a.fights == 1 { "" } else { "s" },
                    a.describe()
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// What `suggest` needs from one analysed fight. Small and serialisable, so
/// analysed public runs can be cached instead of their (huge) event streams.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FightEvidence {
    pub title: String,
    pub kind: FightKind,
    pub spell_evidence: Vec<SpellEvidence>,
}

impl From<&FightReport> for FightEvidence {
    fn from(f: &FightReport) -> Self {
        FightEvidence {
            title: f.title.clone(),
            kind: f.kind.clone(),
            spell_evidence: f.spell_evidence.clone(),
        }
    }
}

/// Bump when analysis changes what goes into `FightEvidence`, so cached
/// evidence from older versions is ignored.
/// v2: Warcraft Logs NPC instances became separate units (casts of several
/// mobs of the same type were merged before). v3: `stopped`. v4: `spikes`.
/// v5: `dispelled_by`. v6: `npcs`.
pub const EVIDENCE_VERSION: u32 = 6;

/// Everything learned from a set of fights, keyed by (instance map ID, spell ID).
#[derive(Clone, Debug, Default)]
pub struct Collected {
    pub spells: BTreeMap<(u32, u32), SpellAgg>,
    /// (instance map ID, pack boss name) -> logged encounter ID.
    pub encounters: BTreeMap<(u32, String), u32>,
    /// (instance, boss name as logged) that matched no pack boss.
    pub unmatched_bosses: BTreeSet<(String, String)>,
    pub fights_used: BTreeMap<Origin, u32>,
    /// Titles of fights in instances the pack doesn't have.
    pub fights_skipped: Vec<String>,
    /// M+ fights by number: (instance map ID, affix IDs, NPC IDs seen), for
    /// affix discovery.
    pub keys: BTreeMap<u32, (u32, Vec<u32>, BTreeSet<u32>)>,
    /// Numbers the fights so evidence can be merged per fight.
    fight_no: u32,
}

fn words(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 4 && w != "the")
        .collect()
}

/// The pack boss a logged boss is. By encounter ID, then exact name
/// (ignoring case), then a single boss sharing a distinctive word, then a
/// single boss within two typos: the journal and the log don't always agree
/// ("Dazar, The First King" vs "King Dazar", "Ziekket" vs "Ziekett").
pub fn match_boss<'a>(bosses: &'a [Boss], log_name: &str, encounter_id: u32) -> Option<&'a Boss> {
    if encounter_id != 0
        && let Some(b) = bosses.iter().find(|b| b.encounter_id == encounter_id)
    {
        return Some(b);
    }
    if let Some(b) = bosses
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(log_name))
    {
        return Some(b);
    }
    let w = words(log_name);
    let mut hits = bosses.iter().filter(|b| !words(&b.name).is_disjoint(&w));
    if let (Some(b), None) = (hits.next(), hits.next()) {
        return Some(b);
    }
    // Small spelling differences ("Ziekket" in the journal, "Ziekett" logged).
    let mut close = bosses
        .iter()
        .filter(|b| edit_distance(&b.name.to_lowercase(), &log_name.to_lowercase()) <= 2);
    match (close.next(), close.next()) {
        (Some(b), None) => Some(b),
        _ => None,
    }
}

/// Levenshtein distance, for near-identical boss names.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != *cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

impl Collected {
    /// Add one fight's evidence. Fights in instances the pack doesn't know are skipped.
    pub fn add(&mut self, pack: &SeasonPack, f: &FightReport, origin: Origin) {
        self.add_evidence(pack, &f.into(), origin);
    }

    pub fn add_evidence(&mut self, pack: &SeasonPack, f: &FightEvidence, origin: Origin) {
        // Resolve the instance and the boss context of the fight.
        let (map_id, bosses, raid_boss): (u32, &[Boss], Option<(String, u32)>) = match &f.kind {
            FightKind::MythicPlus {
                map_id,
                zone,
                bosses: markers,
                ..
            } => {
                // Name fallback only when the map ID is unknown (0): a known
                // map ID that isn't in the pack is another instance.
                let Some(d) = pack.dungeon_by_map(*map_id).or_else(|| {
                    pack.dungeons
                        .iter()
                        .find(|d| *map_id == 0 && d.name.eq_ignore_ascii_case(zone))
                }) else {
                    self.fights_skipped.push(f.title.clone());
                    return;
                };
                for m in markers {
                    self.learn_boss(d.map_id, &d.name, &d.bosses, &m.name, m.encounter_id);
                }
                (d.map_id, &d.bosses, None)
            }
            FightKind::Raid {
                encounter_id, name, ..
            } => {
                let Some(r) = pack
                    .raids
                    .iter()
                    .find(|r| match_boss(&r.bosses, name, *encounter_id).is_some())
                else {
                    self.fights_skipped.push(f.title.clone());
                    return;
                };
                self.learn_boss(r.map_id, &r.name, &r.bosses, name, *encounter_id);
                (r.map_id, &r.bosses, Some((name.clone(), *encounter_id)))
            }
        };
        *self.fights_used.entry(origin).or_default() += 1;
        self.fight_no += 1;
        if let FightKind::MythicPlus { affixes, .. } = &f.kind {
            let npcs = f
                .spell_evidence
                .iter()
                .flat_map(|e| e.npcs.iter().copied())
                .collect();
            self.keys
                .insert(self.fight_no, (map_id, affixes.clone(), npcs));
        }

        for e in &f.spell_evidence {
            let boss = match (&raid_boss, &e.boss) {
                (Some((n, id)), _) => match_boss(bosses, n, *id).map(|b| b.name.clone()),
                (None, Some(n)) => match_boss(bosses, n, 0).map(|b| b.name.clone()),
                (None, None) => None,
            };
            let s = self.spells.entry((map_id, e.spell_id)).or_default();
            if s.name.is_empty() {
                s.name.clone_from(&e.name);
            }
            *s.boss_votes.entry(boss).or_default() += 1;
            s.seen.push((self.fight_no, origin, e.clone()));
        }
    }

    fn learn_boss(
        &mut self,
        map_id: u32,
        inst: &str,
        bosses: &[Boss],
        log_name: &str,
        encounter_id: u32,
    ) {
        match match_boss(bosses, log_name, encounter_id) {
            Some(b) if encounter_id != 0 => {
                self.encounters
                    .insert((map_id, b.name.clone()), encounter_id);
            }
            Some(_) => {}
            None => {
                self.unmatched_bosses
                    .insert((inst.to_string(), log_name.to_string()));
            }
        }
    }
}

/// What `apply` changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub spells_added: usize,
    pub ids_added: usize,
    pub encounter_ids_filled: usize,
    /// Spell entries whose `dispel` types were filled in or narrowed.
    pub dispel_set: usize,
    /// Debuffs whose dispels share no type (the class table needs a look).
    pub dispel_conflicts: Vec<String>,
    /// Affix mechanics found: (spell name, affix IDs).
    pub affix_spells: Vec<(String, Vec<u32>)>,
    /// Affix mechanics left alone: a reviewed per-dungeon entry already holds
    /// the ID, or the affix isn't in the pack.
    pub affix_skipped: Vec<String>,
    /// (instance, boss/trash, spell, new suggestions, evidence) for the review table.
    pub proposals: Vec<(String, String, String, Vec<Tag>, String)>,
}

/// Merge learned IDs and proposals into the pack. Never touches `tags`,
/// `note` or `dismissed`; `suggested` and `evidence` are rewritten for every
/// spell seen in this run.
pub fn apply(pack: &mut SeasonPack, c: &Collected) -> Applied {
    let mut out = Applied::default();
    let inst_name = |pack: &SeasonPack, map: u32| -> String {
        pack.dungeons
            .iter()
            .map(|d| (d.map_id, &d.name))
            .chain(pack.raids.iter().map(|r| (r.map_id, &r.name)))
            .find(|(m, _)| *m == map)
            .map(|(_, n)| n.clone())
            .unwrap_or_default()
    };

    for ((map, boss), id) in &c.encounters {
        for b in pack
            .dungeons
            .iter_mut()
            .filter(|d| d.map_id == *map)
            .flat_map(|d| d.bosses.iter_mut())
            .chain(
                pack.raids
                    .iter_mut()
                    .filter(|r| r.map_id == *map)
                    .flat_map(|r| r.bosses.iter_mut()),
            )
        {
            if b.name == *boss && b.encounter_id == 0 {
                b.encounter_id = *id;
                out.encounter_ids_filled += 1;
            }
        }
    }

    // Pass 0: affix mechanics get (or move to) an entry with map_id = 0.
    let affix_ev = affix_mechanics(c);
    for (id, (name, affixes, ev)) in &affix_ev {
        let known: Vec<u32> = affixes
            .iter()
            .copied()
            .filter(|a| pack.affixes.iter().any(|x| x.id == *a))
            .collect();
        if known.is_empty() {
            out.affix_skipped.push(format!(
                "{name} ({id}): affix {affixes:?} is not in the pack's [[affix]] list"
            ));
            continue;
        }
        match pack.spells.iter().position(|e| e.ids.contains(id)) {
            Some(i) if pack.spells[i].map_id == 0 => {
                if pack.spells[i].affixes.is_empty() {
                    pack.spells[i].affixes.clone_from(&known);
                }
            }
            Some(i) if reviewed(&pack.spells[i]) => {
                out.affix_skipped.push(format!(
                    "{name} ({id}): reviewed per-dungeon entry {:?} kept; move it by hand if it's {ev}",
                    pack.spells[i].name
                ));
                continue;
            }
            found => {
                // A tool-made per-dungeon entry: the ID moves to the affix entry.
                if let Some(i) = found {
                    pack.spells[i].ids.retain(|x| x != id);
                    if pack.spells[i].ids.is_empty() {
                        pack.spells.remove(i);
                    }
                }
                match pack
                    .spells
                    .iter()
                    .position(|e| e.map_id == 0 && e.name == *name && e.affixes == known)
                {
                    Some(i) => pack.spells[i].ids.push(*id),
                    None => pack.spells.push(SpellEntry {
                        ids: vec![*id],
                        name: name.clone(),
                        affixes: known.clone(),
                        ..Default::default()
                    }),
                }
            }
        }
        out.affix_spells.push((name.clone(), known));
    }

    // Pass 1: find or create the pack entry for every spell ID seen enough.
    let mut per_entry: BTreeMap<usize, Vec<&SpellAgg>> = BTreeMap::new();
    for ((map, id), s) in &c.spells {
        if s.total().events() < MIN_EVENTS {
            continue;
        }
        let boss = s.boss();
        let idx = if let Some(i) = pack.spells.iter().position(|e| e.ids.contains(id)) {
            i
        } else if let Some(i) = pack.spells.iter().position(|e| {
            e.map_id == *map
                && !s.name.is_empty()
                && e.name == s.name
                && (e.boss.is_none() || boss.is_none() || e.boss == boss)
        }) {
            pack.spells[i].ids.push(*id);
            out.ids_added += 1;
            i
        } else {
            pack.spells.push(SpellEntry {
                ids: vec![*id],
                name: s.name.clone(),
                map_id: *map,
                boss: boss.clone(),
                ..Default::default()
            });
            out.spells_added += 1;
            pack.spells.len() - 1
        };
        per_entry.entry(idx).or_default().push(s);
    }

    // Pass 2: judge each entry on the combined evidence of all its IDs.
    for (idx, parts) in per_entry {
        let s = SpellAgg::merge(&parts);
        let map = pack.spells[idx].map_id;
        let inst = inst_name(pack, map);
        let e = &mut pack.spells[idx];
        match s.dispel_types() {
            Some(t) if t.is_empty() => out.dispel_conflicts.push(e.name.clone()),
            Some(t) => {
                // Only ever narrowed: more dispels can only rule types out.
                let new: Vec<DispelType> = if e.dispel.is_empty() {
                    t
                } else {
                    e.dispel.iter().copied().filter(|x| t.contains(x)).collect()
                };
                if new.is_empty() {
                    out.dispel_conflicts.push(e.name.clone());
                } else if new != e.dispel {
                    e.dispel = new;
                    out.dispel_set += 1;
                }
            }
            None => {}
        }
        for o in s.by().keys() {
            if !e.sources.contains(&o.source()) {
                e.sources.push(o.source());
            }
        }
        let mut proposed: Vec<Tag> = s.propose();
        let affix_note = e.ids.iter().find_map(|id| affix_ev.get(id)).map(|x| &x.2);
        if e.map_id == 0 && !e.affixes.is_empty() && affix_note.is_some() {
            proposed.push(Tag::Affix);
        }
        // A reviewed kick tier stands: don't propose the other one.
        let tiered = e.tags.contains(&Tag::PriorityKick) || e.tags.contains(&Tag::Kick);
        proposed.retain(|t| {
            !e.tags.contains(t)
                && !e.dismissed.contains(t)
                && !(tiered && matches!(t, Tag::PriorityKick | Tag::Kick))
        });
        let new: Vec<Tag> = proposed
            .iter()
            .copied()
            .filter(|t| !e.suggested.contains(t))
            .collect();
        e.suggested = proposed;
        e.evidence = s.evidence();
        if let Some(n) = affix_note {
            e.evidence = format!("{}; {n}", e.evidence);
        }
        if !new.is_empty() {
            out.proposals.push((
                inst,
                e.boss.clone().unwrap_or_else(|| "trash".into()),
                e.name.clone(),
                new,
                e.evidence.clone(),
            ));
        }
    }
    crate::season::sort(pack);
    out.proposals.sort();
    out.affix_spells.sort();
    out.affix_skipped.sort();
    out.dispel_conflicts.sort();
    out.dispel_conflicts.dedup();
    out
}

/// A reviewer has worked on this entry, so tools don't move its IDs.
fn reviewed(e: &SpellEntry) -> bool {
    !e.tags.is_empty() || !e.note.is_empty() || !e.dismissed.is_empty()
}

/// Spell ID -> (name, affix IDs, evidence) for spells that come from an
/// affix: seen enough, only in keys that have the affix, from units that
/// never show up in a key without it, with at least `MIN_CONTROL_KEYS` keys
/// of the same dungeon(s) without the affix, and in at least `AFFIX_PRESENCE`
/// of those dungeons' keys with it. When several of the key's
/// affixes qualify (Tyrannical alongside a Bargain), the rarest wins.
pub fn affix_mechanics(c: &Collected) -> BTreeMap<u32, (String, Vec<u32>, String)> {
    let mut by_id: BTreeMap<u32, Vec<(u32, &SpellAgg)>> = BTreeMap::new();
    for ((map, id), s) in &c.spells {
        by_id.entry(*id).or_default().push((*map, s));
    }
    let keys_with = |a: u32| c.keys.values().filter(|k| k.1.contains(&a)).count();
    let mut out = BTreeMap::new();
    for (id, parts) in by_id {
        let s = SpellAgg::merge(&parts.iter().map(|p| p.1).collect::<Vec<_>>());
        let npcs = s.npcs();
        if s.total().events() < MIN_EVENTS || npcs.is_empty() {
            continue;
        }
        let fights: BTreeSet<u32> = s.seen.iter().map(|x| x.0).collect();
        let Some(keys) = fights
            .iter()
            .map(|f| c.keys.get(f))
            .collect::<Option<Vec<_>>>()
        else {
            continue; // seen in a raid: not an affix
        };
        let maps: BTreeSet<u32> = parts.iter().map(|p| p.0).collect();
        let mut common: Vec<u32> = keys[0].1.clone();
        common.retain(|a| keys.iter().all(|k| k.1.contains(a)));
        let mut ok: Vec<(usize, u32, usize, usize)> = vec![];
        for a in common {
            let without: Vec<&(u32, Vec<u32>, BTreeSet<u32>)> =
                c.keys.values().filter(|k| !k.1.contains(&a)).collect();
            if without.iter().any(|k| !k.2.is_disjoint(&npcs)) {
                continue;
            }
            let control = without.iter().filter(|k| maps.contains(&k.0)).count();
            let with = c
                .keys
                .values()
                .filter(|k| maps.contains(&k.0) && k.1.contains(&a))
                .count();
            // An affix in most keys (Guile, in every key from +12) is
            // confounded with key level and route: nothing to compare against.
            if control >= MIN_CONTROL_KEYS.max(with)
                && keys.len() as f64 >= AFFIX_PRESENCE * with as f64
            {
                ok.push((keys_with(a), a, control, with));
            }
        }
        let Some(rarest) = ok.iter().map(|x| x.0).min() else {
            continue;
        };
        let best: Vec<&(usize, u32, usize, usize)> = ok.iter().filter(|x| x.0 == rarest).collect();
        let affixes: Vec<u32> = best.iter().map(|x| x.1).collect();
        let control = best.iter().map(|x| x.2).min().unwrap_or(0);
        let with = best.iter().map(|x| x.3).max().unwrap_or(0);
        let ev = format!(
            "affix {}: in {} of {with} key{} with it, its casters in none of {control} key{} without it in the same dungeon",
            affixes
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join("+"),
            keys.len(),
            if with == 1 { "" } else { "s" },
            if control == 1 { "" } else { "s" },
        );
        out.insert(id, (s.name.clone(), affixes, ev));
    }
    out
}

/// Console review: new proposals grouped by instance, then the summary.
pub fn review(c: &Collected, a: &Applied) -> String {
    let mut o = String::new();
    let mut last = String::new();
    for (inst, boss, name, tags, ev) in &a.proposals {
        if *inst != last {
            o.push_str(&format!("\n{inst}\n"));
            last.clone_from(inst);
        }
        let tags: Vec<&str> = tags.iter().map(|t| tag_name(*t)).collect();
        o.push_str(&format!(
            "  {name} ({boss}) -> {}\n      {ev}\n",
            tags.join(", ")
        ));
    }
    let used: Vec<String> = c
        .fights_used
        .iter()
        .map(|(o, n)| format!("{n} from {}", o.label()))
        .collect();
    o.push_str(&format!(
        "\nFights used: {}. Spells added: {}. IDs added to existing spells: {}. Encounter IDs filled: {}. New proposals: {}.\n",
        if used.is_empty() { "none".into() } else { used.join(", ") },
        a.spells_added,
        a.ids_added,
        a.encounter_ids_filled,
        a.proposals.len()
    ));
    if a.dispel_set > 0 {
        o.push_str(&format!(
            "Dispel types set or narrowed on {} spell(s).\n",
            a.dispel_set
        ));
    }
    for n in &a.dispel_conflicts {
        o.push_str(&format!(
            "{n}: its dispels share no type; check their types with `wowcoach classes check`.\n"
        ));
    }
    for (n, ids) in &a.affix_spells {
        o.push_str(&format!("Affix mechanic: {n} (affix {ids:?}).\n"));
    }
    for n in &a.affix_skipped {
        o.push_str(&format!("Affix mechanic not moved: {n}.\n"));
    }
    if !c.fights_skipped.is_empty() {
        o.push_str(&format!(
            "Skipped {} fight(s) in instances not in the pack (e.g. {}).\n",
            c.fights_skipped.len(),
            c.fights_skipped[0]
        ));
    }
    for (inst, boss) in &c.unmatched_bosses {
        o.push_str(&format!(
            "Boss {boss:?} in {inst} matched no pack boss: add its encounter_id by hand.\n"
        ));
    }
    o
}

#[cfg(test)]
mod tests;
