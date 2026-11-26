//! Season data pack: the dungeons, raids and enemy spells of one M+ season /
//! raid tier, with human-reviewed tags ("must kick", "avoidable", ...).
//!
//! Packs live in `data/seasons/*.toml` and the current one is compiled into
//! the binary. `season sync` (Blizzard API) and `season suggest` (logs) fill
//! in IDs; `tags` are only ever set by a reviewer. Rules never read the pack
//! directly: they ask [`crate::config::Config`] (`has_tag`, `avoidable_note`).
//! See `docs/milestones/m1-season-data.md`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Packs compiled into the binary, oldest first. The last one is the default.
pub const EMBEDDED: &[(&str, &str)] = &[(
    "midnight-s2",
    include_str!("../../../data/seasons/midnight-s2.toml"),
)];

/// What a reviewer can say about an enemy spell. Unknown tags are a load
/// error, so a typo can't silently disable a rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    /// Must be interrupted every time.
    PriorityKick,
    /// Crowd control (stun, incapacitate, knock) also stops the cast.
    Stoppable,
    /// Should never hit anyone.
    Avoidable,
    /// Dispel the debuff as soon as it lands.
    DispelPriority,
    /// Big hit on the tank: needs active mitigation or a defensive.
    TankBuster,
    /// Group-wide damage event: needs group defensives.
    DangerousCast,
    /// Comes from an affix rather than the dungeon itself.
    Affix,
}

/// Where a spell entry's IDs were learned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Blizzard Encounter Journal.
    Journal,
    /// The user's own combat logs.
    Log,
    /// Public Warcraft Logs reports.
    Wcl,
    /// Added by hand.
    Manual,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeasonInfo {
    /// Short identifier, e.g. "midnight-s2".
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blizzard_season_id: Option<u32>,
    /// Game build the pack was last synced against, e.g. "12.1.5".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_build: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Boss {
    pub name: String,
    /// Encounter Journal ID (Blizzard API). Not what the combat log uses.
    #[serde(default)]
    pub journal_id: u32,
    /// `ENCOUNTER_START` ID as logged (and as used by Warcraft Logs). 0 = not known yet.
    #[serde(default)]
    pub encounter_id: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dungeon {
    pub name: String,
    /// challengeModeID: 4th field of `CHALLENGE_MODE_START`.
    pub keystone_id: u32,
    /// instanceID: 3rd field of `CHALLENGE_MODE_START`, also the journal `map.id`.
    pub map_id: u32,
    #[serde(default)]
    pub journal_id: u32,
    /// Seconds for +1, +2, +3.
    #[serde(default)]
    pub timer_s: Vec<u32>,
    #[serde(default)]
    pub bosses: Vec<Boss>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Raid {
    pub name: String,
    pub map_id: u32,
    #[serde(default)]
    pub journal_id: u32,
    #[serde(default)]
    pub bosses: Vec<Boss>,
}

/// One enemy mechanic. A mechanic is often logged under several spell IDs
/// (different casters, or the cast vs. the damage), so `ids` is a list.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpellEntry {
    pub ids: Vec<u32>,
    #[serde(default)]
    pub name: String,
    /// Instance (`map_id`) the spell belongs to; 0 = not tied to one (e.g. affixes).
    #[serde(default)]
    pub map_id: u32,
    /// Boss name; absent = trash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boss: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
    /// Reviewed tags. Only these drive rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    /// Tags proposed by `season suggest`, waiting for review. Ignored by rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested: Vec<Tag>,
    /// Short advice shown to players, e.g. "Step out of the line".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Tags a reviewer rejected: `season suggest` won't propose them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<Tag>,
    /// Counts behind the latest suggestion, written by `season suggest` for
    /// reviewers. Not shown to players.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    /// User overrides only: switch this pack entry off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeasonPack {
    pub season: SeasonInfo,
    #[serde(default, rename = "dungeon")]
    pub dungeons: Vec<Dungeon>,
    #[serde(default, rename = "raid")]
    pub raids: Vec<Raid>,
    #[serde(default, rename = "spell")]
    pub spells: Vec<SpellEntry>,
}

impl SeasonPack {
    /// Parse and validate. All problems are reported at once.
    pub fn parse(text: &str) -> anyhow::Result<SeasonPack> {
        let pack: SeasonPack = toml::from_str(text)?;
        let problems = pack.problems();
        if !problems.is_empty() {
            anyhow::bail!("invalid season pack:\n  - {}", problems.join("\n  - "));
        }
        Ok(pack)
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<SeasonPack> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// The newest pack compiled into the binary.
    pub fn embedded() -> SeasonPack {
        let (key, text) = EMBEDDED.last().expect("at least one embedded pack");
        Self::parse(text).unwrap_or_else(|e| panic!("embedded pack {key} is invalid: {e}"))
    }

    /// Everything wrong with the pack, as human-readable lines.
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if self.season.key.trim().is_empty() {
            out.push("[season] key is empty".into());
        }

        let mut maps: BTreeMap<u32, (&str, Vec<&Boss>)> = BTreeMap::new();
        let mut keystones = HashSet::new();
        for d in &self.dungeons {
            if d.map_id == 0 {
                out.push(format!("dungeon {:?} has map_id = 0", d.name));
            }
            if d.keystone_id == 0 || !keystones.insert(d.keystone_id) {
                out.push(format!(
                    "dungeon {:?}: keystone_id {} is 0 or duplicated",
                    d.name, d.keystone_id
                ));
            }
            if maps
                .insert(d.map_id, (&d.name, d.bosses.iter().collect()))
                .is_some()
            {
                out.push(format!(
                    "map_id {} is used by more than one instance",
                    d.map_id
                ));
            }
        }
        for r in &self.raids {
            if r.map_id == 0 {
                out.push(format!("raid {:?} has map_id = 0", r.name));
            }
            if maps
                .insert(r.map_id, (&r.name, r.bosses.iter().collect()))
                .is_some()
            {
                out.push(format!(
                    "map_id {} is used by more than one instance",
                    r.map_id
                ));
            }
        }

        let mut encounters: HashMap<u32, &str> = HashMap::new();
        for (inst, bosses) in maps.values() {
            let mut names = HashSet::new();
            for b in bosses {
                if b.name.trim().is_empty() {
                    out.push(format!("{inst:?} has a boss with no name"));
                }
                if !names.insert(b.name.as_str()) {
                    out.push(format!("{inst:?} lists boss {:?} twice", b.name));
                }
                if b.encounter_id != 0
                    && let Some(other) = encounters.insert(b.encounter_id, &b.name)
                {
                    out.push(format!(
                        "encounter_id {} is used by both {other:?} and {:?}",
                        b.encounter_id, b.name
                    ));
                }
            }
        }

        let mut seen: HashMap<u32, &str> = HashMap::new();
        for s in &self.spells {
            let label = if s.name.is_empty() {
                format!("spell {:?}", s.ids)
            } else {
                format!("spell {:?}", s.name)
            };
            if s.ids.is_empty() {
                out.push(format!("{label} has no ids"));
            }
            if s.exclude {
                out.push(format!(
                    "{label}: `exclude` only belongs in a user override"
                ));
            }
            for id in &s.ids {
                if *id == 0 {
                    out.push(format!("{label} has spell id 0"));
                } else if let Some(other) = seen.insert(*id, &s.name) {
                    out.push(format!(
                        "spell id {id} appears in both {other:?} and {:?}",
                        s.name
                    ));
                }
            }
            if s.map_id != 0 {
                match maps.get(&s.map_id) {
                    None => out.push(format!(
                        "{label}: map_id {} is not a dungeon or raid in this pack",
                        s.map_id
                    )),
                    Some((inst, bosses)) => {
                        if let Some(b) = &s.boss
                            && !bosses.iter().any(|x| &x.name == b)
                        {
                            out.push(format!("{label}: boss {b:?} is not in {inst:?}"));
                        }
                    }
                }
            } else if s.boss.is_some() {
                out.push(format!("{label}: a boss needs a map_id"));
            }
            for t in &s.suggested {
                if s.tags.contains(t) {
                    out.push(format!("{label}: {t:?} is both tagged and suggested"));
                }
                if s.dismissed.contains(t) {
                    out.push(format!("{label}: {t:?} is both dismissed and suggested"));
                }
            }
        }
        out
    }

    /// Dungeon for a `CHALLENGE_MODE_START` map ID.
    pub fn dungeon_by_map(&self, map_id: u32) -> Option<&Dungeon> {
        self.dungeons.iter().find(|d| d.map_id == map_id)
    }

    /// Counts for `season check`: dungeons, raids, bosses, spells, and spells per tag.
    pub fn summary(&self) -> String {
        let bosses: usize = self.dungeons.iter().map(|d| d.bosses.len()).sum::<usize>()
            + self.raids.iter().map(|r| r.bosses.len()).sum::<usize>();
        let known = self
            .dungeons
            .iter()
            .flat_map(|d| &d.bosses)
            .chain(self.raids.iter().flat_map(|r| &r.bosses))
            .filter(|b| b.encounter_id != 0)
            .count();
        let mut per_tag: BTreeMap<Tag, usize> = BTreeMap::new();
        for s in &self.spells {
            for t in &s.tags {
                *per_tag.entry(*t).or_default() += 1;
            }
        }
        let tags = if per_tag.is_empty() {
            "none".to_string()
        } else {
            per_tag
                .iter()
                .map(|(t, n)| format!("{} {n}", tag_name(*t)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pending = self
            .spells
            .iter()
            .filter(|s| !s.suggested.is_empty())
            .count();
        format!(
            "{} ({}): {} dungeons, {} raids, {bosses} bosses ({known} with encounter IDs), {} spells. Tagged: {tags}. Awaiting review: {pending}.",
            self.season.name,
            self.season.key,
            self.dungeons.len(),
            self.raids.len(),
            self.spells.len(),
        )
    }
}

pub fn tag_name(t: Tag) -> &'static str {
    match t {
        Tag::PriorityKick => "priority_kick",
        Tag::Stoppable => "stoppable",
        Tag::Avoidable => "avoidable",
        Tag::DispelPriority => "dispel_priority",
        Tag::TankBuster => "tank_buster",
        Tag::DangerousCast => "dangerous_cast",
        Tag::Affix => "affix",
    }
}

/// Resolved spell knowledge: the pack plus user overrides, keyed by spell ID.
#[derive(Clone, Debug, Default)]
pub struct Knowledge {
    /// Key of the pack in use, for the report ("midnight-s2"), if any.
    pub pack_key: Option<String>,
    pub pack: Option<SeasonPack>,
    by_id: HashMap<u32, KnownSpell>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KnownSpell {
    pub name: String,
    pub note: String,
    pub tags: Vec<Tag>,
}

impl Knowledge {
    /// Combine a pack with user `[[spell]]` overrides. An override with
    /// `exclude = true` removes those IDs; otherwise its tags are added and
    /// its name/note replace the pack's when set.
    pub fn build(pack: Option<SeasonPack>, overrides: &[SpellEntry]) -> Knowledge {
        let mut by_id: HashMap<u32, KnownSpell> = HashMap::new();
        if let Some(p) = &pack {
            for s in &p.spells {
                let k = KnownSpell {
                    name: s.name.clone(),
                    note: s.note.clone(),
                    tags: s.tags.clone(),
                };
                for id in &s.ids {
                    by_id.insert(*id, k.clone());
                }
            }
        }
        for o in overrides {
            for id in &o.ids {
                if o.exclude {
                    by_id.remove(id);
                    continue;
                }
                let k = by_id.entry(*id).or_default();
                if !o.name.is_empty() {
                    k.name.clone_from(&o.name);
                }
                if !o.note.is_empty() {
                    k.note.clone_from(&o.note);
                }
                for t in &o.tags {
                    if !k.tags.contains(t) {
                        k.tags.push(*t);
                    }
                }
            }
        }
        Knowledge {
            pack_key: pack.as_ref().map(|p| p.season.key.clone()),
            pack,
            by_id,
        }
    }

    pub fn get(&self, id: u32) -> Option<&KnownSpell> {
        self.by_id.get(&id)
    }

    pub fn has_tag(&self, id: u32, tag: Tag) -> bool {
        self.by_id.get(&id).is_some_and(|k| k.tags.contains(&tag))
    }
}

/// Deterministic order so re-syncing gives clean diffs: instances by name,
/// spells by instance, then boss (journal order, trash last), then name.
pub fn sort(pack: &mut SeasonPack) {
    pack.dungeons.sort_by(|a, b| a.name.cmp(&b.name));
    pack.raids.sort_by(|a, b| a.name.cmp(&b.name));
    let mut inst_order: HashMap<u32, (usize, Vec<String>)> = HashMap::new();
    for (i, (map, bosses)) in pack
        .dungeons
        .iter()
        .map(|d| (d.map_id, &d.bosses))
        .chain(pack.raids.iter().map(|r| (r.map_id, &r.bosses)))
        .enumerate()
    {
        inst_order.insert(map, (i, bosses.iter().map(|b| b.name.clone()).collect()));
    }
    for s in pack.spells.iter_mut() {
        s.ids.sort_unstable();
        s.ids.dedup();
        s.sources.sort();
        s.sources.dedup();
    }
    pack.spells.sort_by_cached_key(|s| {
        let (inst, bosses) = inst_order
            .get(&s.map_id)
            .map(|(i, b)| (*i, b.as_slice()))
            .unwrap_or((usize::MAX, &[]));
        let boss = s
            .boss
            .as_ref()
            .and_then(|b| bosses.iter().position(|x| x == b))
            .unwrap_or(usize::MAX);
        (
            inst,
            boss,
            s.name.clone(),
            s.ids.first().copied().unwrap_or(0),
        )
    });
}

/// Render a pack as TOML, keeping the comment block at the top of the
/// previous file (`header`).
pub fn render(pack: &SeasonPack, previous: &str) -> anyhow::Result<String> {
    let header: String = previous
        .lines()
        .take_while(|l| l.starts_with('#') || l.trim().is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    let body = toml::to_string(pack)?;
    let header = header.trim();
    Ok(if header.is_empty() {
        body
    } else {
        format!("{header}\n\n{}", body.trim_start())
    })
}

#[cfg(test)]
mod tests;
