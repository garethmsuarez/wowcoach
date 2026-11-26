// Some flag constants are only used by the Warcraft Logs adapter.
#![cfg_attr(not(feature = "wcl"), allow(dead_code))]
//! Source-agnostic event model. Both the raw combat-log parser and the
//! Warcraft Logs adapter produce these, so analysis and rules only exist once.

// Unit flag bits (COMBATLOG_OBJECT_*)
pub const AFFILIATION_MINE: u32 = 0x1;
pub const AFFILIATION_PARTY: u32 = 0x2;
pub const AFFILIATION_RAID: u32 = 0x4;
pub const REACTION_FRIENDLY: u32 = 0x10;
pub const REACTION_HOSTILE: u32 = 0x40;
pub const TYPE_PLAYER: u32 = 0x400;
pub const TYPE_NPC: u32 = 0x800;
pub const TYPE_PET: u32 = 0x1000;

#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub guid: String,
    pub name: String,
    pub flags: u32,
}

impl Unit {
    pub fn is_player(&self) -> bool {
        self.guid.starts_with("Player-")
    }
    /// A player in our party/raid (excludes random outsiders in the open world).
    pub fn is_group_player(&self) -> bool {
        self.is_player()
            && self.flags & (AFFILIATION_MINE | AFFILIATION_PARTY | AFFILIATION_RAID) != 0
    }
    pub fn is_hostile(&self) -> bool {
        self.flags & REACTION_HOSTILE != 0
    }
    pub fn is_hostile_npc(&self) -> bool {
        self.is_hostile() && !self.is_player()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spell {
    pub id: u32,
    pub name: String,
}

impl Spell {
    pub fn melee() -> Self {
        Spell {
            id: 1,
            name: "Melee".into(),
        }
    }
}

/// "Advanced combat logging" unit snapshot attached to some events.
#[derive(Clone, Debug, PartialEq)]
pub struct AdvInfo {
    pub guid: String,
    pub owner_guid: String,
    pub hp: u64,
    pub max_hp: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Damage {
        spell: Spell,
        amount: u64,
        overkill: u64,
        absorbed: u64,
        periodic: bool,
        info: Option<AdvInfo>,
    },
    Heal {
        spell: Spell,
        amount: u64,
        overheal: u64,
        absorbed: u64,
        info: Option<AdvInfo>,
    },
    CastStart {
        spell: Spell,
    },
    CastSuccess {
        spell: Spell,
        info: Option<AdvInfo>,
    },
    CastFailed {
        spell: Spell,
    },
    Interrupt {
        spell: Spell,
        extra: Spell,
    },
    /// A shield/absorb on `dst` soaked `amount` of damage from `src`.
    /// `absorber` cast the shield and gets credit for the healing.
    Absorbed {
        absorber: Unit,
        spell: Spell,
        amount: u64,
    },
    Dispel {
        spell: Spell,
        extra: Spell,
    },
    AuraApplied {
        spell: Spell,
        is_buff: bool,
    },
    AuraRemoved {
        spell: Spell,
        is_buff: bool,
    },
    Summon {
        spell: Spell,
    },
    Death,
    Combatant {
        guid: String,
        spec_id: u32,
    },
    EncounterStart {
        id: u32,
        name: String,
        difficulty: u32,
        group_size: u32,
    },
    EncounterEnd {
        id: u32,
        name: String,
        difficulty: u32,
        success: bool,
        duration_ms: i64,
    },
    ChallengeStart {
        zone: String,
        map_id: u32,
        level: u32,
        affixes: Vec<u32>,
    },
    ZoneChange {
        instance_id: u32,
    },
    ChallengeEnd {
        map_id: u32,
        success: bool,
        level: u32,
        time_ms: i64,
    },
}

#[derive(Clone, Debug)]
pub struct Event {
    /// Milliseconds on a monotonic-ish clock (only differences matter).
    pub ts: i64,
    pub src: Option<Unit>,
    pub dst: Option<Unit>,
    pub kind: Kind,
}
