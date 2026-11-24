//! Tunable thresholds and spell knowledge. Ships with sensible defaults;
//! dump them with `wowcoach init-config` and extend per season/tier.

use serde::{Deserialize, Serialize};

pub const DEFAULT_CONFIG: &str = r#"# wowcoach configuration
# Spell IDs change every season/tier: add the avoidable mechanics and
# must-kick casts for the dungeons and bosses you're running.

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

# Mechanics that should never hit anyone (or never hit the wrong role).
# [[avoidable]]
# spell_id = 123456
# name = "Example Swirlies"
# note = "Move out of the orange circles"

# Enemy casts that must always be interrupted/stopped.
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
}

impl Config {
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Config> {
        let text = match path {
            Some(p) => std::fs::read_to_string(p)?,
            None => DEFAULT_CONFIG.to_string(),
        };
        Ok(toml::from_str(&text)?)
    }
    pub fn avoidable_note(&self, id: u32) -> Option<&SpellNote> {
        self.avoidable.iter().find(|s| s.spell_id == id)
    }
    pub fn is_priority_kick(&self, id: u32) -> bool {
        self.priority_interrupt.iter().any(|s| s.spell_id == id)
    }
}
