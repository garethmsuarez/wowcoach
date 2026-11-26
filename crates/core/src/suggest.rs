//! `wowcoach season suggest`: learn spell IDs, boss encounter IDs and tag
//! *proposals* from analysed fights (your own logs and/or top public keys on
//! Warcraft Logs), and merge them into a season pack.
//!
//! Tools never set `tags`. Proposals go to `suggested` with the counts behind
//! them in `evidence`; a reviewer promotes or dismisses them. Only spell IDs,
//! names and aggregate counts are kept: no player names.

use crate::analysis::{FightKind, FightReport, SpellEvidence};
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
/// `priority_kick`: kicked at least this often...
pub const MIN_KICKS: u32 = 3;
/// ...and at least this share of completed-or-kicked cast bars was kicked.
pub const KICK_RATE: f64 = 0.5;
/// `avoidable`: flagged by the uneven-hit heuristic in at least half of the
/// fights where it hit twice or more, in at least this many fights.
pub const MIN_AVOIDABLE_FIGHTS: u32 = 2;
/// `dispel_priority`: dispelled at least this often, and this share of applications.
pub const MIN_DISPELS: u32 = 3;
pub const DISPEL_RATE: f64 = 0.4;
/// `tank_buster`: this share of its damage lands on tanks, it has a cast bar,
/// and it hit at least `MIN_EVENTS` times.
pub const TANK_SHARE: f64 = 0.9;

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
        if kicks.interrupted >= MIN_KICKS && kicks.kick_rate() >= KICK_RATE {
            out.push(Tag::PriorityKick);
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
        out
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
/// mobs of the same type were merged before).
pub const EVIDENCE_VERSION: u32 = 2;

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
        for o in s.by().keys() {
            if !e.sources.contains(&o.source()) {
                e.sources.push(o.source());
            }
        }
        let proposed: Vec<Tag> = s
            .propose()
            .into_iter()
            .filter(|t| !e.tags.contains(t) && !e.dismissed.contains(t))
            .collect();
        let new: Vec<Tag> = proposed
            .iter()
            .copied()
            .filter(|t| !e.suggested.contains(t))
            .collect();
        e.suggested = proposed;
        e.evidence = s.evidence();
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
