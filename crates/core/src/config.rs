//! Tunable thresholds and spell knowledge. Ships with sensible defaults;
//! dump them with `wowcoach init-config`.
//!
//! Spell knowledge is layered: the season pack compiled into the binary (or
//! `--season-pack PATH`), then the user's `[[spell]]` overrides, then the
//! older `[[avoidable]]` / `[[priority_interrupt]]` lists.

use crate::season::{Knowledge, SeasonPack, SpellEntry, Tag};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r#"# wowcoach configuration
# Season knowledge (dungeons, bosses, must-kick and avoidable spells) ships in
# the built-in season pack: see `wowcoach season check`. Use this file for
# thresholds and to adjust the pack for your group.

[general]
pull_gap_ms = 5000          # M+: no group combat for this long ends a pull
gcd_ms = 1300               # assumed GCD for activity estimation
activity_warn = 0.80        # flag DPS/healers below this estimated activity
dps_warn_ratio = 0.70       # flag DPS below this fraction of the group DPS median
overheal_warn = 0.45        # flag healers above this overheal share
death_penalty_s = 15        # M+ timer penalty per death
early_death_frac = 0.30     # raid wipe: first death in the first 30% of the pull is "early"
heuristic_min_share = 0.02  # enemy spell must be >=2% of group damage taken to be considered

# Bloodlust / Heroism / Time Warp / Primal Rage (+ hunter pet version) / Fury of the Aspects
lust_spell_ids = [2825, 32182, 80353, 264667, 272678, 390386]
# Healthstone, Demonic Healthstone
healthstone_spell_ids = [6262, 452930]
# Add the current expansion's healing potion spell IDs here
potion_spell_ids = []
# Absorbs that only delay damage (not healing): Stagger, Purgatory
absorb_exclude_spell_ids = [115069, 114556]
# The delayed damage those produce later; excluded from damage-taken totals
# (it was already counted when the original hit was absorbed). Stagger ticks.
deferred_damage_spell_ids = [124255, 324393]

# Adjust the season pack. Tags are added to the pack's; name/note replace it.
# Tags: priority_kick, stoppable, avoidable, dispel_priority, tank_buster,
#       dangerous_cast, affix
# [[spell]]
# ids = [123456]
# name = "Example Swirlies"
# tags = ["avoidable"]
# note = "Move out of the orange circles"
#
# Switch a pack entry off:
# [[spell]]
# ids = [654321]
# exclude = true

# Older shorthand, still supported (same as tags = ["avoidable"] / ["priority_kick"]):
# [[avoidable]]
# spell_id = 123456
# name = "Example Swirlies"
# note = "Move out of the orange circles"
# [[priority_interrupt]]
# spell_id = 654321
# name = "Example Bolt"
"#;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct General {
    pub pull_gap_ms: i64,
    pub gcd_ms: i64,
    pub activity_warn: f64,
    pub dps_warn_ratio: f64,
    pub overheal_warn: f64,
    pub death_penalty_s: i64,
    pub early_death_frac: f64,
    pub heuristic_min_share: f64,
    pub lust_spell_ids: Vec<u32>,
    pub healthstone_spell_ids: Vec<u32>,
    pub potion_spell_ids: Vec<u32>,
    pub absorb_exclude_spell_ids: Vec<u32>,
    pub deferred_damage_spell_ids: Vec<u32>,
}

impl Default for General {
    fn default() -> Self {
        General {
            pull_gap_ms: 5000,
            gcd_ms: 1300,
            activity_warn: 0.80,
            dps_warn_ratio: 0.70,
            overheal_warn: 0.45,
            death_penalty_s: 15,
            early_death_frac: 0.30,
            heuristic_min_share: 0.02,
            lust_spell_ids: vec![2825, 32182, 80353, 264667, 272678, 390386],
            healthstone_spell_ids: vec![6262, 452930],
            potion_spell_ids: vec![],
            absorb_exclude_spell_ids: vec![115069, 114556],
            deferred_damage_spell_ids: vec![124255, 324393],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpellNote {
    pub spell_id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub avoidable: Vec<SpellNote>,
    pub priority_interrupt: Vec<SpellNote>,
    /// User overrides of the season pack.
    #[serde(rename = "spell")]
    pub spell_overrides: Vec<SpellEntry>,
    /// Pack + overrides, resolved by spell ID. Built by [`Config::load_with`].
    #[serde(skip)]
    pub knowledge: Knowledge,
}

/// Which season pack to use.
#[derive(Clone, Debug, PartialEq)]
pub enum PackChoice {
    /// The newest pack compiled into the binary.
    Embedded,
    /// A pack file, e.g. while editing one (`--season-pack PATH`).
    File(PathBuf),
    /// No pack: only the user config (`--no-season-pack`).
    None,
}

/// Name and advice for a known spell, from the pack or the user config.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note<'a> {
    pub name: &'a str,
    pub note: &'a str,
}

impl Config {
    /// User config (or the defaults) plus the embedded season pack.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Config> {
        Self::load_with(path, PackChoice::Embedded)
    }

    pub fn load_with(path: Option<&Path>, pack: PackChoice) -> anyhow::Result<Config> {
        let text = match path {
            Some(p) => std::fs::read_to_string(p)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?,
            None => DEFAULT_CONFIG.to_string(),
        };
        let mut cfg: Config = toml::from_str(&text)?;
        let pack = match pack {
            PackChoice::Embedded => Some(SeasonPack::embedded()),
            PackChoice::File(p) => Some(SeasonPack::load(&p)?),
            PackChoice::None => None,
        };
        cfg.knowledge = Knowledge::build(pack, &cfg.spell_overrides);
        Ok(cfg)
    }

    /// Is this spell tagged `tag` by the pack, a `[[spell]]` override or the
    /// older `[[avoidable]]` / `[[priority_interrupt]]` lists?
    pub fn has_tag(&self, id: u32, tag: Tag) -> bool {
        let legacy = match tag {
            Tag::Avoidable => self.avoidable.iter().any(|s| s.spell_id == id),
            Tag::PriorityKick => self.priority_interrupt.iter().any(|s| s.spell_id == id),
            _ => false,
        };
        legacy || self.knowledge.has_tag(id, tag)
    }

    /// Name and advice if the spell is avoidable.
    pub fn avoidable_note(&self, id: u32) -> Option<Note<'_>> {
        if let Some(s) = self.avoidable.iter().find(|s| s.spell_id == id) {
            return Some(Note {
                name: &s.name,
                note: &s.note,
            });
        }
        self.knowledge
            .get(id)
            .filter(|k| k.tags.contains(&Tag::Avoidable))
            .map(|k| Note {
                name: &k.name,
                note: &k.note,
            })
    }

    pub fn is_priority_kick(&self, id: u32) -> bool {
        self.has_tag(id, Tag::PriorityKick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> SeasonPack {
        SeasonPack::parse(
            r#"
            [season]
            key = "test"
            name = "Test"
            [[dungeon]]
            name = "Spire"
            keystone_id = 1
            map_id = 2000
            [[spell]]
            ids = [10, 11]
            name = "Void Pool"
            map_id = 2000
            tags = ["avoidable"]
            note = "Move"
            [[spell]]
            ids = [20]
            name = "Shadow Volley"
            map_id = 2000
            tags = ["priority_kick", "stoppable"]
            [[spell]]
            ids = [30]
            name = "Maybe"
            map_id = 2000
            suggested = ["avoidable"]
            "#,
        )
        .unwrap()
    }

    fn cfg(user: &str, pack: Option<SeasonPack>) -> Config {
        let mut c: Config = toml::from_str(user).unwrap();
        c.knowledge = Knowledge::build(pack, &c.spell_overrides);
        c
    }

    #[test]
    fn pack_tags_reach_the_rule_helpers() {
        let c = cfg("", Some(pack()));
        assert_eq!(
            c.avoidable_note(11),
            Some(Note {
                name: "Void Pool",
                note: "Move"
            })
        );
        assert!(c.is_priority_kick(20));
        assert!(c.has_tag(20, Tag::Stoppable));
        assert!(!c.is_priority_kick(10));
        // Suggestions are not tags until a reviewer promotes them.
        assert!(c.avoidable_note(30).is_none());
        assert!(c.avoidable_note(999).is_none());
    }

    #[test]
    fn user_overrides_add_replace_and_exclude() {
        let c = cfg(
            r#"
            [[spell]]
            ids = [20]
            exclude = true
            [[spell]]
            ids = [10]
            note = "Our strat: soak it"
            tags = ["dangerous_cast"]
            [[spell]]
            ids = [40]
            name = "New Thing"
            tags = ["priority_kick"]
            "#,
            Some(pack()),
        );
        assert!(!c.is_priority_kick(20));
        assert!(!c.has_tag(20, Tag::Stoppable));
        // Overrides are per ID: 11 keeps the pack's note.
        assert_eq!(c.avoidable_note(10).unwrap().note, "Our strat: soak it");
        assert_eq!(c.avoidable_note(11).unwrap().note, "Move");
        assert!(c.has_tag(10, Tag::Avoidable) && c.has_tag(10, Tag::DangerousCast));
        assert!(c.is_priority_kick(40));
    }

    #[test]
    fn legacy_lists_still_work_without_a_pack() {
        let c = cfg(
            r#"
            [[avoidable]]
            spell_id = 88
            name = "Swirl"
            note = "Dodge"
            [[priority_interrupt]]
            spell_id = 77
            "#,
            None,
        );
        assert_eq!(c.avoidable_note(88).unwrap().note, "Dodge");
        assert!(c.is_priority_kick(77));
        assert!(c.knowledge.pack_key.is_none());
    }

    #[test]
    fn default_config_text_parses_with_the_embedded_pack() {
        let c = Config::load(None).unwrap();
        assert_eq!(c.knowledge.pack_key.as_deref(), Some("midnight-s2"));
        assert_eq!(c.general.pull_gap_ms, General::default().pull_gap_ms);
        let none = Config::load_with(None, PackChoice::None).unwrap();
        assert!(none.knowledge.pack_key.is_none());
    }

    #[test]
    fn unknown_tag_is_an_error() {
        let e =
            toml::from_str::<Config>("[[spell]]\nids = [1]\ntags = [\"avoidble\"]\n").unwrap_err();
        assert!(e.to_string().contains("avoidble"), "{e}");
    }
}
