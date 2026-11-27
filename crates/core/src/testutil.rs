//! Builders for hand-made event streams in unit tests. Keeps tests short and
//! readable: `ev.damage(t, &boss, &tank, 9, "Cleave", 50_000)` instead of a
//! dozen struct fields.

use crate::model::*;

pub const TANK_SPEC: u32 = 250; // Blood DK
pub const HEALER_SPEC: u32 = 256; // Discipline
pub const DPS_SPEC: u32 = 62; // Arcane

pub fn player(name: &str) -> Unit {
    Unit {
        guid: format!("Player-1-{name}"),
        name: format!("{name}-Realm"),
        flags: TYPE_PLAYER | AFFILIATION_PARTY | REACTION_FRIENDLY,
    }
}

pub fn enemy(name: &str, n: u32) -> Unit {
    Unit {
        guid: format!("Creature-0-1-1-1-{n}-0000{n:06}"),
        name: name.to_string(),
        flags: TYPE_NPC | REACTION_HOSTILE,
    }
}

pub fn pet(name: &str, n: u32) -> Unit {
    Unit {
        guid: format!("Pet-0-1-1-1-{n}-0000{n:06}"),
        name: name.to_string(),
        flags: TYPE_PET | AFFILIATION_PARTY | REACTION_FRIENDLY,
    }
}

pub fn spell(id: u32, name: &str) -> Spell {
    Spell {
        id,
        name: name.to_string(),
        school: 0,
    }
}

fn ev(ts: i64, src: Option<&Unit>, dst: Option<&Unit>, kind: Kind) -> Event {
    Event {
        ts,
        src: src.cloned(),
        dst: dst.cloned(),
        kind,
    }
}

/// Event-stream builder. Times are in milliseconds.
#[derive(Default)]
pub struct Log {
    pub events: Vec<Event>,
}

impl Log {
    pub fn push(
        &mut self,
        ts: i64,
        src: Option<&Unit>,
        dst: Option<&Unit>,
        kind: Kind,
    ) -> &mut Self {
        self.events.push(ev(ts, src, dst, kind));
        self
    }
    pub fn combatant(&mut self, ts: i64, u: &Unit, spec_id: u32) -> &mut Self {
        self.push(
            ts,
            None,
            None,
            Kind::Combatant {
                guid: u.guid.clone(),
                spec_id,
            },
        )
    }
    pub fn damage(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        id: u32,
        name: &str,
        amount: u64,
    ) -> &mut Self {
        self.damage_hp(ts, src, dst, id, name, amount, None)
    }
    /// Damage with an advanced-logging snapshot of `dst` (hp / max_hp after the hit).
    #[allow(clippy::too_many_arguments)]
    pub fn damage_hp(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        id: u32,
        name: &str,
        amount: u64,
        hp: Option<(u64, u64)>,
    ) -> &mut Self {
        let info = hp.map(|(hp, max_hp)| AdvInfo {
            guid: dst.guid.clone(),
            owner_guid: "0000000000000000".into(),
            hp,
            max_hp,
            pos: None,
        });
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Damage {
                spell: spell(id, name),
                amount,
                overkill: 0,
                absorbed: 0,
                periodic: false,
                info,
            },
        )
    }
    pub fn heal(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        amount: u64,
        overheal: u64,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Heal {
                spell: spell(2061, "Flash Heal"),
                amount,
                overheal,
                absorbed: 0,
                info: None,
            },
        )
    }
    pub fn cast_start(&mut self, ts: i64, src: &Unit, id: u32, name: &str) -> &mut Self {
        self.push(
            ts,
            Some(src),
            None,
            Kind::CastStart {
                spell: spell(id, name),
            },
        )
    }
    pub fn cast(&mut self, ts: i64, src: &Unit, id: u32, name: &str) -> &mut Self {
        self.push(
            ts,
            Some(src),
            None,
            Kind::CastSuccess {
                spell: spell(id, name),
                info: None,
            },
        )
    }
    pub fn interrupt(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        kicked_id: u32,
        kicked: &str,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Interrupt {
                spell: spell(1766, "Kick"),
                extra: spell(kicked_id, kicked),
            },
        )
    }
    /// Harmful aura (debuff) from `src` on `dst`.
    pub fn debuff(&mut self, ts: i64, src: &Unit, dst: &Unit, id: u32, name: &str) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::AuraApplied {
                spell: spell(id, name),
                is_buff: false,
            },
        )
    }
    /// Helpful aura (buff) from `src` on `dst`.
    pub fn buff(&mut self, ts: i64, src: &Unit, dst: &Unit, id: u32, name: &str) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::AuraApplied {
                spell: spell(id, name),
                is_buff: true,
            },
        )
    }
    /// An aura from `src` fades from `dst`.
    pub fn fade(&mut self, ts: i64, src: &Unit, dst: &Unit, id: u32, is_buff: bool) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::AuraRemoved {
                spell: spell(id, "aura"),
                is_buff,
            },
        )
    }
    /// Damage of a given school, with `dst`'s max HP in the advanced info.
    #[allow(clippy::too_many_arguments)]
    pub fn hit(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        id: u32,
        school: u32,
        amount: u64,
        max_hp: u64,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Damage {
                spell: Spell {
                    school,
                    ..spell(id, &format!("Spell {id}"))
                },
                amount,
                overkill: 0,
                absorbed: 0,
                periodic: false,
                info: Some(AdvInfo {
                    guid: dst.guid.clone(),
                    owner_guid: "0000000000000000".into(),
                    hp: max_hp / 2,
                    max_hp,
                    pos: None,
                }),
            },
        )
    }
    /// `src` dispels aura `removed_id` from `dst`.
    pub fn dispel(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        removed_id: u32,
        removed: &str,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Dispel {
                spell: spell(527, "Purify"),
                extra: spell(removed_id, removed),
            },
        )
    }
    /// `src` dispels aura `removed_id` from `dst` with a given dispel.
    #[allow(clippy::too_many_arguments)]
    pub fn dispel_with(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        with: u32,
        with_name: &str,
        removed_id: u32,
        removed: &str,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Dispel {
                spell: spell(with, with_name),
                extra: spell(removed_id, removed),
            },
        )
    }
    /// An event that logs `u`'s position (a cast with advanced info).
    pub fn at(&mut self, ts: i64, u: &Unit, x: f32, y: f32) -> &mut Self {
        self.push(
            ts,
            Some(u),
            None,
            Kind::CastSuccess {
                spell: spell(999_999, "Position"),
                info: Some(AdvInfo {
                    guid: u.guid.clone(),
                    owner_guid: "0000000000000000".into(),
                    hp: 1,
                    max_hp: 1,
                    pos: Some(Pos { x, y, map: 1 }),
                }),
            },
        )
    }
    /// `src` interrupts `dst`'s `kicked` cast with its own `with` spell.
    #[allow(clippy::too_many_arguments)]
    pub fn kick_with(
        &mut self,
        ts: i64,
        src: &Unit,
        dst: &Unit,
        with: u32,
        with_name: &str,
        kicked: u32,
        kicked_name: &str,
    ) -> &mut Self {
        self.push(
            ts,
            Some(src),
            Some(dst),
            Kind::Interrupt {
                spell: spell(with, with_name),
                extra: spell(kicked, kicked_name),
            },
        )
    }
    pub fn death(&mut self, ts: i64, u: &Unit) -> &mut Self {
        self.push(ts, None, Some(u), Kind::Death)
    }
    pub fn summon(&mut self, ts: i64, owner: &Unit, pet: &Unit) -> &mut Self {
        self.push(
            ts,
            Some(owner),
            Some(pet),
            Kind::Summon {
                spell: spell(883, "Call Pet"),
            },
        )
    }
    pub fn encounter_start(
        &mut self,
        ts: i64,
        id: u32,
        name: &str,
        difficulty: u32,
        size: u32,
    ) -> &mut Self {
        self.push(
            ts,
            None,
            None,
            Kind::EncounterStart {
                id,
                name: name.into(),
                difficulty,
                group_size: size,
            },
        )
    }
    pub fn encounter_end(
        &mut self,
        ts: i64,
        id: u32,
        name: &str,
        difficulty: u32,
        success: bool,
    ) -> &mut Self {
        self.push(
            ts,
            None,
            None,
            Kind::EncounterEnd {
                id,
                name: name.into(),
                difficulty,
                success,
                duration_ms: 0,
            },
        )
    }
    pub fn key_start(&mut self, ts: i64, zone: &str, map_id: u32, level: u32) -> &mut Self {
        self.push(
            ts,
            None,
            None,
            Kind::ChallengeStart {
                zone: zone.into(),
                map_id,
                level,
                affixes: vec![],
            },
        )
    }
    pub fn key_end(&mut self, ts: i64, map_id: u32, success: bool, time_ms: i64) -> &mut Self {
        self.push(
            ts,
            None,
            None,
            Kind::ChallengeEnd {
                map_id,
                success,
                level: 0,
                time_ms,
            },
        )
    }
    pub fn zone(&mut self, ts: i64, instance_id: u32) -> &mut Self {
        self.push(ts, None, None, Kind::ZoneChange { instance_id })
    }
}

/// A standard 5-player party: tank, healer and three DPS, with specs logged.
pub struct Party {
    pub tank: Unit,
    pub healer: Unit,
    pub dps: [Unit; 3],
}

impl Party {
    pub fn new() -> Self {
        Party {
            tank: player("Tank"),
            healer: player("Healer"),
            dps: [player("Alpha"), player("Bravo"), player("Charlie")],
        }
    }
    pub fn all(&self) -> Vec<&Unit> {
        let mut v = vec![&self.tank, &self.healer];
        v.extend(self.dps.iter());
        v
    }
    pub fn log_specs(&self, log: &mut Log, ts: i64) {
        log.combatant(ts, &self.tank, TANK_SPEC)
            .combatant(ts, &self.healer, HEALER_SPEC);
        for d in &self.dps {
            log.combatant(ts, d, DPS_SPEC);
        }
    }
}

/// Run events through a fresh analyzer with the given config.
pub fn analyze(log: &Log, cfg: crate::config::Config) -> Vec<crate::analysis::FightReport> {
    let mut an = crate::analysis::Analyzer::new(cfg);
    for e in &log.events {
        an.feed(e.clone());
    }
    an.finish().0
}
