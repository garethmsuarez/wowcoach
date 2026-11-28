//! Warcraft Logs v2 (GraphQL) adapter.
//!
//! Pulls fights, actors, abilities, events and rankings for a report and
//! converts them into the same `Event` stream the raw-log parser produces,
//! so analysis and rules are shared. Mapping is a pure function (tested
//! offline); network access needs a client from
//! https://www.warcraftlogs.com/api/clients/ .

use crate::model::*;
use serde_json::Value;
use std::collections::HashMap;

pub const TOKEN_URL: &str = "https://www.warcraftlogs.com/oauth/token";
pub const API_URL: &str = "https://www.warcraftlogs.com/api/v2/client";

#[derive(Debug, Clone)]
pub struct Actor {
    pub name: String,
    pub kind: String,
    pub pet_owner: Option<i64>,
    /// NPC ID for NPCs (`gameID`).
    pub game_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FightMeta {
    pub id: i64,
    pub encounter_id: u32,
    pub name: String,
    pub start: i64,
    pub end: i64,
    pub kill: bool,
    pub boss_pct: Option<f64>,
    pub difficulty: u32,
    pub size: u32,
    pub keystone_level: Option<u32>,
    pub keystone_time: Option<i64>,
    pub keystone_bonus: Option<i64>,
    pub affixes: Vec<u32>,
    /// Instance map ID (`gameZone.id`), = `CHALLENGE_MODE_START` instanceID.
    pub game_zone: u32,
    /// Boss pulls inside a keystone run (`dungeonPulls` with an encounter ID).
    pub boss_pulls: Vec<BossPull>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BossPull {
    /// Logged `ENCOUNTER_START` ID.
    pub encounter_id: u32,
    pub name: String,
    pub start: i64,
    pub end: i64,
    pub kill: bool,
}

#[derive(Default)]
pub struct Master {
    pub actors: HashMap<i64, Actor>,
    /// Ability game ID -> (name, school bitmask; 0 when WCL gives no type).
    pub abilities: HashMap<i64, (String, u32)>,
}

fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}
fn u(v: &Value, k: &str) -> u64 {
    i(v, k).unwrap_or(0).max(0) as u64
}

impl Master {
    pub fn from_json(md: &Value) -> Master {
        let mut m = Master::default();
        for a in md["actors"].as_array().into_iter().flatten() {
            if let Some(id) = i(a, "id") {
                m.actors.insert(
                    id,
                    Actor {
                        name: a["name"].as_str().unwrap_or("?").to_string(),
                        kind: a["type"].as_str().unwrap_or("NPC").to_string(),
                        pet_owner: i(a, "petOwner").filter(|o| *o > 0),
                        game_id: i(a, "gameID").filter(|g| *g > 0),
                    },
                );
            }
        }
        for a in md["abilities"].as_array().into_iter().flatten() {
            if let Some(id) = i(a, "gameID") {
                // `type` is the spell school bitmask, sent as a string ("32").
                let school = match &a["type"] {
                    Value::String(s) => s.parse().unwrap_or(0),
                    v => v.as_u64().unwrap_or(0) as u32,
                };
                m.abilities
                    .insert(id, (a["name"].as_str().unwrap_or("?").to_string(), school));
            }
        }
        m
    }

    /// WCL has one actor per NPC *type*; individual mobs are told apart by
    /// `sourceInstance` / `targetInstance` (absent = 1). Each gets its own GUID,
    /// shaped like the game's (`Creature-0-…-NPCID-spawn`) so the NPC ID sits in
    /// the same field.
    fn guid(&self, id: i64, instance: Option<i64>) -> String {
        let a = self.actors.get(&id);
        match a.map(|a| a.kind.as_str()) {
            Some("Player") => format!("Player-WCL-{id}"),
            Some("Pet") => format!("Pet-WCL-{id}"),
            _ => format!(
                "Creature-0-WCL-{id}-0-{}-{}",
                a.and_then(|a| a.game_id).unwrap_or(0),
                instance.unwrap_or(1)
            ),
        }
    }

    fn unit(&self, id: Option<i64>, friendly: Option<bool>) -> Option<Unit> {
        self.unit_n(id, friendly, None)
    }

    fn unit_n(
        &self,
        id: Option<i64>,
        friendly: Option<bool>,
        instance: Option<i64>,
    ) -> Option<Unit> {
        let id = id.filter(|x| *x >= 0)?;
        let a = self.actors.get(&id);
        let kind = a.map(|a| a.kind.as_str()).unwrap_or("NPC");
        let friendly = friendly.unwrap_or(kind == "Player" || kind == "Pet");
        let mut flags = if friendly {
            REACTION_FRIENDLY | AFFILIATION_RAID
        } else {
            REACTION_HOSTILE
        };
        flags |= match kind {
            "Player" => TYPE_PLAYER,
            "Pet" => TYPE_PET,
            _ => TYPE_NPC,
        };
        Some(Unit {
            guid: self.guid(id, instance),
            name: a
                .map(|a| a.name.clone())
                .unwrap_or_else(|| format!("#{id}")),
            flags,
        })
    }

    fn spell(&self, id: Option<i64>) -> Spell {
        let id = id.unwrap_or(0);
        if id == 1 {
            return Spell::melee();
        }
        let (name, school) = self
            .abilities
            .get(&id)
            .cloned()
            .unwrap_or_else(|| (format!("Spell {id}"), 0));
        Spell {
            id: id.max(0) as u32,
            name,
            school,
        }
    }

    /// Synthetic summon events so the analyzer attributes pet damage.
    pub fn ownership_events(&self, ts: i64) -> Vec<Event> {
        self.actors
            .iter()
            .filter_map(|(id, a)| {
                let o = a.pet_owner?;
                Some(Event {
                    ts,
                    src: self.unit(Some(o), Some(true)),
                    dst: self.unit(Some(*id), Some(true)),
                    kind: Kind::Summon {
                        spell: Spell {
                            id: 0,
                            name: "owner".into(),
                            school: 0,
                        },
                    },
                })
            })
            .collect()
    }

    /// Convert one WCL event object into our model.
    pub fn convert(&self, e: &Value) -> Option<Event> {
        let ts = i(e, "timestamp")?;
        let src = self.unit_n(
            i(e, "sourceID"),
            e.get("sourceIsFriendly").and_then(Value::as_bool),
            i(e, "sourceInstance"),
        );
        let dst = self.unit_n(
            i(e, "targetID"),
            e.get("targetIsFriendly").and_then(Value::as_bool),
            i(e, "targetInstance"),
        );
        let ability = self.spell(i(e, "abilityGameID"));
        let info = || -> Option<AdvInfo> {
            let max = u(e, "maxHitPoints");
            if max == 0 {
                return None;
            }
            // resourceActor: 1 = source, 2 = target
            let who = if i(e, "resourceActor") == Some(1) {
                &src
            } else {
                &dst
            };
            // WCL map coordinates are hundredths of a yard with the axes
            // swapped: raw x = WCL y / 100, raw y = -WCL x / 100 (checked
            // against raw logs of the same dungeon).
            let pos = match (e["x"].as_f64(), e["y"].as_f64(), i(e, "mapID")) {
                (Some(x), Some(y), Some(map)) if map > 0 => Some(Pos {
                    x: (y / 100.0) as f32,
                    y: (-x / 100.0) as f32,
                    map: map as u32,
                }),
                _ => None,
            };
            Some(AdvInfo {
                guid: who.as_ref()?.guid.clone(),
                owner_guid: "0000000000000000".into(),
                hp: u(e, "hitPoints"),
                max_hp: max,
                pos,
            })
        };
        let kind = match e["type"].as_str()? {
            "combatantinfo" => Kind::Combatant {
                guid: src.as_ref()?.guid.clone(),
                spec_id: u(e, "specID") as u32,
            },
            "damage" => Kind::Damage {
                spell: ability,
                amount: u(e, "amount"),
                overkill: u(e, "overkill"),
                absorbed: u(e, "absorbed"),
                periodic: e["tick"].as_bool().unwrap_or(false),
                info: info(),
            },
            // WCL heal amount excludes overheal; our model's includes it.
            "heal" => Kind::Heal {
                spell: ability,
                amount: u(e, "amount") + u(e, "overheal"),
                overheal: u(e, "overheal"),
                absorbed: u(e, "absorbed"),
                info: info(),
            },
            // sourceID = shield caster, targetID = protected unit, attackerID = attacker
            "absorbed" => {
                let absorber = self.unit(i(e, "sourceID"), Some(true))?;
                let attacker = self.unit(i(e, "attackerID"), None);
                return Some(Event {
                    ts,
                    src: attacker,
                    dst,
                    kind: Kind::Absorbed {
                        absorber,
                        spell: ability,
                        amount: u(e, "amount"),
                    },
                });
            }
            "begincast" => Kind::CastStart { spell: ability },
            "cast" => Kind::CastSuccess {
                spell: ability,
                info: None,
            },
            "interrupt" => Kind::Interrupt {
                spell: ability,
                extra: self.spell(i(e, "extraAbilityGameID")),
            },
            "dispel" => Kind::Dispel {
                spell: ability,
                extra: self.spell(i(e, "extraAbilityGameID")),
            },
            "applybuff" => Kind::AuraApplied {
                spell: ability,
                is_buff: true,
            },
            "applydebuff" => Kind::AuraApplied {
                spell: ability,
                is_buff: false,
            },
            "removebuff" => Kind::AuraRemoved {
                spell: ability,
                is_buff: true,
            },
            "removedebuff" => Kind::AuraRemoved {
                spell: ability,
                is_buff: false,
            },
            "summon" => Kind::Summon { spell: ability },
            "death" => {
                let killer = self.unit(i(e, "killerID"), None);
                return Some(Event {
                    ts,
                    src: killer,
                    dst,
                    kind: Kind::Death,
                });
            }
            _ => return None,
        };
        Some(Event { ts, src, dst, kind })
    }
}

/// Map WCL difficulty to in-game DifficultyID.
fn wcl_difficulty(d: u32) -> u32 {
    match d {
        1 => 17,
        3 => 14,
        4 => 15,
        5 => 16,
        10 => 8,
        x => x,
    }
}

impl FightMeta {
    pub fn from_json(f: &Value) -> Option<FightMeta> {
        Some(FightMeta {
            id: i(f, "id")?,
            encounter_id: i(f, "encounterID").unwrap_or(0) as u32,
            name: f["name"].as_str().unwrap_or("?").to_string(),
            start: i(f, "startTime")?,
            end: i(f, "endTime")?,
            kill: f["kill"].as_bool().unwrap_or(false),
            boss_pct: f["bossPercentage"].as_f64(),
            difficulty: i(f, "difficulty").unwrap_or(0) as u32,
            size: i(f, "size").unwrap_or(0) as u32,
            keystone_level: i(f, "keystoneLevel").filter(|k| *k > 0).map(|k| k as u32),
            keystone_time: i(f, "keystoneTime"),
            keystone_bonus: i(f, "keystoneBonus"),
            affixes: f["keystoneAffixes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|x| x as u32)
                .collect(),
            game_zone: f["gameZone"]["id"].as_u64().unwrap_or(0) as u32,
            boss_pulls: f["dungeonPulls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some(BossPull {
                        encounter_id: i(p, "encounterID").filter(|id| *id > 0)? as u32,
                        name: p["name"].as_str().unwrap_or("?").to_string(),
                        start: i(p, "startTime")?,
                        end: i(p, "endTime")?,
                        kill: p["kill"].as_bool().unwrap_or(false),
                    })
                })
                .collect(),
        })
    }

    /// Instance map ID: `gameZone`, else decoded from a keystone fight's
    /// encounter ID (WCL uses prefix * 10000 + map ID, e.g. 12923 -> 2923).
    pub fn map_id(&self) -> u32 {
        if self.game_zone > 0 {
            self.game_zone
        } else if self.keystone_level.is_some() {
            self.encounter_id % 10_000
        } else {
            0
        }
    }

    /// `EncounterStart/End` for each boss inside a keystone run, so M+ boss
    /// markers (and their encounter IDs) match the raw-log path.
    pub fn boss_events(&self) -> Vec<Event> {
        if self.keystone_level.is_none() {
            return vec![];
        }
        let mut out = vec![];
        for b in &self.boss_pulls {
            let ev = |ts, kind| Event {
                ts,
                src: None,
                dst: None,
                kind,
            };
            out.push(ev(
                b.start,
                Kind::EncounterStart {
                    id: b.encounter_id,
                    name: b.name.clone(),
                    difficulty: 8,
                    group_size: self.size,
                },
            ));
            out.push(ev(
                b.end,
                Kind::EncounterEnd {
                    id: b.encounter_id,
                    name: b.name.clone(),
                    difficulty: 8,
                    success: b.kill,
                    duration_ms: b.end - b.start,
                },
            ));
        }
        out
    }

    pub fn is_interesting(&self) -> bool {
        self.encounter_id > 0 || self.keystone_level.is_some()
    }

    pub fn start_event(&self) -> Event {
        let kind = match self.keystone_level {
            Some(level) => Kind::ChallengeStart {
                zone: self.name.clone(),
                map_id: self.map_id(),
                level,
                affixes: self.affixes.clone(),
            },
            None => Kind::EncounterStart {
                id: self.encounter_id,
                name: self.name.clone(),
                difficulty: wcl_difficulty(self.difficulty),
                group_size: self.size,
            },
        };
        Event {
            ts: self.start,
            src: None,
            dst: None,
            kind,
        }
    }

    pub fn end_event(&self) -> Event {
        let kind = match self.keystone_level {
            Some(level) => Kind::ChallengeEnd {
                map_id: self.map_id(),
                // Completed, like the raw log's flag; timed or not is
                // judged from the time against the pack's timer.
                success: self.kill || self.keystone_bonus.is_some_and(|b| b > 0),
                level,
                time_ms: self.keystone_time.unwrap_or(self.end - self.start),
            },
            None => Kind::EncounterEnd {
                id: self.encounter_id,
                name: self.name.clone(),
                difficulty: wcl_difficulty(self.difficulty),
                success: self.kill,
                duration_ms: self.end - self.start,
            },
        };
        Event {
            ts: self.end,
            src: None,
            dst: None,
            kind,
        }
    }
}

/// (boss name, encounter ID) for every encounter in the current (not frozen,
/// not PTR/Beta) zones of a `worldData.expansions.zones` response. WCL encounter IDs for
/// raid bosses are the IDs `ENCOUNTER_START` logs, unlike Blizzard's journal IDs.
pub fn zone_encounters(v: &Value) -> Vec<(String, u32)> {
    let mut out = vec![];
    for x in v["worldData"]["expansions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for z in x["zones"].as_array().into_iter().flatten() {
            let name = z["name"].as_str().unwrap_or_default();
            if z["frozen"].as_bool().unwrap_or(false)
                || name.contains("(PTR)")
                || name.contains("(Beta)")
            {
                continue;
            }
            for e in z["encounters"].as_array().into_iter().flatten() {
                if let (Some(name), Some(id)) = (e["name"].as_str(), e["id"].as_u64()) {
                    out.push((name.to_string(), id as u32));
                }
            }
        }
    }
    out
}

/// A ranked keystone run with a public report.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedKey {
    pub code: String,
    pub fight_id: i64,
    pub level: u32,
}

/// Public runs from an `encounter.fightRankings` blob (anonymous runs have an
/// empty report code and are skipped), and whether more pages exist.
pub fn parse_fight_rankings(v: &Value) -> (Vec<RankedKey>, bool) {
    let keys = v["rankings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let code = r["report"]["code"].as_str().filter(|c| !c.is_empty())?;
            Some(RankedKey {
                code: code.to_string(),
                fight_id: r["report"]["fightID"].as_i64()?,
                level: r["bracketData"].as_u64().unwrap_or(0) as u32,
            })
        })
        .collect();
    (keys, v["hasMorePages"].as_bool().unwrap_or(false))
}

/// One public keystone run, reduced to game data for `analyze --compare-top`:
/// `fight` is an entry of a report's `fights` (with `dungeonPulls`), `actors`
/// the report's `masterData.actors` (NPCs, for their game IDs), `deaths` the
/// enemy death events. None for fights that aren't keystone runs.
pub fn ref_run(
    code: &str,
    fight: &Value,
    actors: &Value,
    deaths: &[Value],
) -> Option<crate::reference::RefRun> {
    use crate::reference::{RefPull, RefRun};
    let meta = FightMeta::from_json(fight)?;
    let level = meta.keystone_level?;
    let game: HashMap<i64, u32> = actors
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((i(a, "id")?, i(a, "gameID")? as u32)))
        .collect();
    let rel = |t: i64| t - meta.start;
    let mut kills: Vec<(i64, u32)> = deaths
        .iter()
        .filter(|e| e["type"] == "death")
        .filter_map(|e| Some((rel(i(e, "timestamp")?), *game.get(&i(e, "targetID")?)?)))
        .collect();
    kills.sort();
    Some(RefRun {
        code: code.to_string(),
        fight_id: meta.id,
        map_id: meta.map_id(),
        level,
        affixes: meta.affixes.clone(),
        time_ms: meta.keystone_time.unwrap_or(meta.end - meta.start),
        bonus: meta.keystone_bonus.map(|b| b.clamp(0, 3) as u8),
        pulls: fight["dungeonPulls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(RefPull {
                    start_ms: rel(i(p, "startTime")?),
                    end_ms: rel(i(p, "endTime")?),
                    encounter_id: i(p, "encounterID").unwrap_or(0).max(0) as u32,
                    kill: p["kill"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
        kills,
        non_tanks: 0,
        damage: vec![],
    })
}

/// A run's non-tank damage taken for `RefRun`: (non-tank players, damage per
/// spell). `details` is `report.playerDetails` for the fight, `npcs` the
/// report's NPC actor IDs and `events` its `DamageTaken` events for friendlies.
/// Only hits from an NPC or the environment (actor -1) count, as in the
/// analyzer's `enemy_damage`. None when the roles are missing.
pub fn ref_damage(
    details: &Value,
    npcs: &std::collections::HashSet<i64>,
    events: &[Value],
) -> Option<(u32, Vec<crate::reference::RefDamage>)> {
    let d = &details["data"]["playerDetails"];
    let ids = |role: &str| -> Vec<i64> {
        d[role]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| i(p, "id"))
            .collect()
    };
    let tanks = ids("tanks");
    let mut non_tanks = ids("healers");
    non_tanks.extend(ids("dps"));
    non_tanks.retain(|p| !tanks.contains(p));
    non_tanks.sort();
    non_tanks.dedup();
    if non_tanks.is_empty() {
        return None;
    }
    let mut by: HashMap<u32, (u32, u64)> = HashMap::new();
    for e in events.iter().filter(|e| e["type"] == "damage") {
        let (Some(t), Some(src), Some(id)) =
            (i(e, "targetID"), i(e, "sourceID"), i(e, "abilityGameID"))
        else {
            continue;
        };
        if !non_tanks.contains(&t) || !(src == -1 || npcs.contains(&src)) {
            continue;
        }
        let n = i(e, "amount").unwrap_or(0).max(0) + i(e, "absorbed").unwrap_or(0).max(0);
        let x = by.entry(id as u32).or_default();
        x.0 += 1;
        x.1 += n as u64;
    }
    let mut damage: Vec<crate::reference::RefDamage> = by
        .into_iter()
        .map(|(spell_id, (hits, amount))| crate::reference::RefDamage {
            spell_id,
            hits,
            amount,
        })
        .collect();
    damage.sort_by_key(|d| d.spell_id);
    Some((non_tanks.len() as u32, damage))
}

/// Order for events sharing a timestamp: boss start first, boss end last.
fn boss_order(e: &Event) -> u8 {
    match e.kind {
        Kind::EncounterStart { .. } => 0,
        Kind::EncounterEnd { .. } => 2,
        _ => 1,
    }
}

/// Merge boss markers into a fight's events in timestamp order (stable).
pub fn with_boss_events(mut events: Vec<Event>, boss: Vec<Event>) -> Vec<Event> {
    events.extend(boss);
    events.sort_by_key(|e| (e.ts, boss_order(e)));
    events
}

/// A player's Warcraft Logs percentiles for one fight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rank {
    /// Against all parses of the spec.
    pub overall: f64,
    /// Against parses at the same key level (or item level bracket).
    pub bracket: Option<f64>,
}

/// name -> percentiles from `report.rankings` fetched twice, with
/// `playerMetric: dps` and `hps`: tanks and DPS by damage, healers by
/// healing. (The default metric of an M+ fight is the key's score, the same
/// for all five players.)
pub fn parse_rankings(dps: &Value, hps: &Value) -> HashMap<String, Rank> {
    let mut out = rankings_for(dps, &["tanks", "dps"]);
    out.extend(rankings_for(hps, &["healers"]));
    out
}

fn rankings_for(v: &Value, roles: &[&str]) -> HashMap<String, Rank> {
    let mut out = HashMap::new();
    for fight in v["data"].as_array().into_iter().flatten() {
        for role in roles {
            for c in fight["roles"][*role]["characters"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let (Some(n), Some(r)) = (c["name"].as_str(), c["rankPercent"].as_f64()) {
                    let bracket = c["bracketPercent"].as_f64();
                    out.insert(
                        n.to_string(),
                        Rank {
                            overall: r,
                            bracket,
                        },
                    );
                }
            }
        }
    }
    out
}

/// "Name-Realm-US" (a raw log's player name) -> (name, realm, region).
pub fn split_player(full: &str) -> Option<(&str, &str, &str)> {
    let mut it = full.rsplitn(3, '-');
    let region = it.next()?;
    let realm = it.next()?;
    let name = it.next()?;
    Some((name, realm, region))
}

/// Server slugs to try for a realm as the log writes it ("SteamwheedleCartel",
/// "Area52", "AzjolNerub"): words split ("steamwheedle-cartel", "area-52"),
/// then joined ("azjolnerub"), which is how realms named with a hyphen end up.
pub fn realm_slugs(realm: &str) -> Vec<String> {
    let mut split = String::new();
    let mut prev: Option<char> = None;
    for ch in realm.chars().filter(|c| c.is_alphanumeric()) {
        if let Some(p) = prev
            && ((p.is_lowercase() && ch.is_uppercase())
                || (p.is_alphabetic() && ch.is_ascii_digit())
                || (p.is_ascii_digit() && ch.is_alphabetic()))
        {
            split.push('-');
        }
        split.extend(ch.to_lowercase());
        prev = Some(ch);
    }
    let joined = split.replace('-', "");
    if joined == split {
        vec![split]
    } else {
        vec![split, joined]
    }
}

/// A character's ranked run from `character.encounterRankings`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedRun {
    pub code: String,
    pub fight_id: i64,
    /// Unix ms (UTC) of the run's start.
    pub start: i64,
    pub level: u32,
}

/// The runs in a `character.encounterRankings` blob.
pub fn ranked_runs(v: &Value) -> Vec<RankedRun> {
    v["ranks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(RankedRun {
                code: r["report"]["code"].as_str()?.to_string(),
                fight_id: i(&r["report"], "fightID")?,
                start: i(r, "startTime")?,
                level: i(r, "bracketData")? as u32,
            })
        })
        .collect()
}

/// How far a ranked run's start may be from the log's `CHALLENGE_MODE_START`.
pub const RUN_MATCH_MS: i64 = 5 * 60_000;

/// The ranked run that is this key: same level, starting within
/// `RUN_MATCH_MS` of `start` (Unix ms), the closest.
pub fn match_run(runs: &[RankedRun], start: i64, level: u32) -> Option<&RankedRun> {
    runs.iter()
        .filter(|r| r.level == level && (r.start - start).abs() <= RUN_MATCH_MS)
        .min_by_key(|r| (r.start - start).abs())
}

// ------------------------------------------------------------- network

#[cfg(feature = "wcl")]
pub mod net {
    use super::*;
    use anyhow::bail;
    use serde_json::json;

    pub struct Client {
        agent: ureq::Agent,
        token: String,
    }

    impl Client {
        pub fn connect(id: &str, secret: &str) -> anyhow::Result<Client> {
            let agent = crate::net::agent();
            let token =
                crate::net::client_credentials(&agent, TOKEN_URL, id, secret, "Warcraft Logs")?;
            Ok(Client { agent, token })
        }

        /// (points spent this hour, hourly limit, seconds until reset).
        pub fn rate_limit(&self) -> anyhow::Result<(f64, f64, i64)> {
            let d = self.query(RATE_Q, json!({}))?;
            let r = &d["rateLimitData"];
            Ok((
                r["pointsSpentThisHour"].as_f64().unwrap_or(0.0),
                r["limitPerHour"].as_f64().unwrap_or(0.0),
                r["pointsResetIn"].as_i64().unwrap_or(0),
            ))
        }

        /// One page of public ranked runs for a WCL encounter (for M+, the
        /// whole-dungeon encounter, e.g. 12923), optionally at one key level.
        /// M+ brackets start at +2 with one level each, so +L is bracket L-1;
        /// callers should still check `RankedKey::level`.
        pub fn top_keys(
            &self,
            encounter_id: u32,
            level: Option<u32>,
            page: u32,
        ) -> anyhow::Result<(Vec<super::RankedKey>, bool)> {
            let d = match level {
                Some(l) => self.query(
                    TOP_BRACKET_Q,
                    json!({ "id": encounter_id, "page": page, "bracket": l.saturating_sub(1) }),
                )?,
                None => self.query(TOP_Q, json!({ "id": encounter_id, "page": page }))?,
            };
            Ok(super::parse_fight_rankings(
                &d["worldData"]["encounter"]["fightRankings"],
            ))
        }

        pub fn query(&self, q: &str, vars: Value) -> anyhow::Result<Value> {
            let resp: Value = self
                .agent
                .post(API_URL)
                .set("Authorization", &format!("Bearer {}", self.token))
                .send_json(json!({ "query": q, "variables": vars }))?
                .into_json()?;
            if let Some(errs) = resp.get("errors") {
                bail!("GraphQL error: {errs}");
            }
            Ok(resp["data"].clone())
        }
    }

    const META_Q: &str = r#"query($code:String!){ reportData{ report(code:$code){
        title startTime
        masterData{ actors{ id name type subType petOwner gameID } abilities{ gameID name type } }
        fights{ id encounterID name startTime endTime kill bossPercentage difficulty size keystoneLevel keystoneTime keystoneBonus keystoneAffixes
                gameZone{ id } dungeonPulls{ encounterID name startTime endTime kill } }
    }}}"#;

    const EVENTS_Q: &str = r#"query($code:String!,$fid:[Int]!,$s:Float!,$e:Float!){ reportData{ report(code:$code){
        events(fightIDs:$fid,startTime:$s,endTime:$e,limit:10000,includeResources:true){ data nextPageTimestamp }
    }}}"#;

    const RANK_Q: &str = r#"query($code:String!,$fid:[Int]!){ reportData{ report(code:$code){
        dps: rankings(fightIDs:$fid, playerMetric: dps) hps: rankings(fightIDs:$fid, playerMetric: hps) } } }"#;

    const CHAR_RANKS_Q: &str = r#"query($n:String!,$s:String!,$r:String!,$e:Int!){ characterData{ character(name:$n, serverSlug:$s, serverRegion:$r){
        encounterRankings(encounterID:$e, difficulty:10, metric:dps) } } }"#;

    const REF_Q: &str = r#"query($code:String!,$fid:[Int]!){ reportData{ report(code:$code){
        masterData{ actors(type:"NPC"){ id gameID } }
        playerDetails(fightIDs:$fid)
        fights(fightIDs:$fid){ id encounterID name startTime endTime kill keystoneLevel keystoneTime keystoneBonus keystoneAffixes
                gameZone{ id } dungeonPulls{ encounterID startTime endTime kill } }
    }}}"#;

    const DEATHS_Q: &str = r#"query($code:String!,$fid:[Int]!,$s:Float!,$e:Float!){ reportData{ report(code:$code){
        events(fightIDs:$fid,startTime:$s,endTime:$e,dataType:Deaths,hostilityType:Enemies,limit:10000){ data nextPageTimestamp }
    }}}"#;

    const DAMAGE_Q: &str = r#"query($code:String!,$fid:[Int]!,$s:Float!,$e:Float!){ reportData{ report(code:$code){
        events(fightIDs:$fid,startTime:$s,endTime:$e,dataType:DamageTaken,hostilityType:Friendlies,limit:10000){ data nextPageTimestamp }
    }}}"#;

    const RATE_Q: &str = r#"{ rateLimitData{ limitPerHour pointsSpentThisHour pointsResetIn } }"#;

    const TOP_Q: &str = r#"query($id:Int!,$page:Int!){ worldData{ encounter(id:$id){ fightRankings(page:$page) } } }"#;

    const TOP_BRACKET_Q: &str = r#"query($id:Int!,$page:Int!,$bracket:Int!){ worldData{ encounter(id:$id){ fightRankings(page:$page, bracket:$bracket) } } }"#;

    const ZONES_Q: &str =
        r#"{ worldData{ expansions{ id name zones{ id name frozen encounters{ id name } } } } }"#;

    impl Client {
        /// name -> Warcraft Logs percentile for one fight: DPS for tanks and
        /// DPS, HPS for healers.
        pub fn fight_rankings(
            &self,
            code: &str,
            fight: i64,
        ) -> anyhow::Result<HashMap<String, super::Rank>> {
            let r = self.query(RANK_Q, json!({ "code": code, "fid": [fight] }))?;
            let rep = &r["reportData"]["report"];
            Ok(super::parse_rankings(&rep["dps"], &rep["hps"]))
        }

        /// Find a key from a raw log on Warcraft Logs, through its players'
        /// rankings for the dungeon (`encounter`, the whole-dungeon ID): the
        /// run at `level` that started within `RUN_MATCH_MS` of `start` (Unix
        /// ms). Players are tried in order (each realm slug in turn); after two
        /// characters found without the run it gives up, since a run is
        /// ranked for all five or for none. Sends names and realms to WCL.
        pub fn find_run(
            &self,
            encounter: u32,
            players: &[String],
            start: i64,
            level: u32,
        ) -> anyhow::Result<Option<super::RankedRun>> {
            let mut found = 0;
            for p in players {
                let Some((name, realm, region)) = super::split_player(p) else {
                    continue;
                };
                for slug in super::realm_slugs(realm) {
                    let d = self.query(
                        CHAR_RANKS_Q,
                        json!({ "n": name, "s": slug, "r": region, "e": encounter }),
                    )?;
                    let ch = &d["characterData"]["character"];
                    if ch.is_null() {
                        continue;
                    }
                    let runs = super::ranked_runs(&ch["encounterRankings"]);
                    if let Some(r) = super::match_run(&runs, start, level) {
                        return Ok(Some(r.clone()));
                    }
                    found += 1;
                    break;
                }
                if found >= 2 {
                    break;
                }
            }
            Ok(None)
        }

        /// Boss name -> logged encounter ID for current content.
        pub fn zone_encounters(&self) -> anyhow::Result<Vec<(String, u32)>> {
            Ok(super::zone_encounters(&self.query(ZONES_Q, json!({}))?))
        }
    }

    /// One public keystone run for `analyze --compare-top`: its pulls, its
    /// enemy deaths and its non-tank damage taken per spell (a few points
    /// each; no other player events).
    pub fn ref_run(c: &Client, code: &str, fight: i64) -> anyhow::Result<crate::reference::RefRun> {
        let d = c.query(REF_Q, json!({ "code": code, "fid": [fight] }))?;
        let report = &d["reportData"]["report"];
        let f = &report["fights"][0];
        let (Some(start), Some(end)) = (f["startTime"].as_f64(), f["endTime"].as_f64()) else {
            bail!("fight {fight} not found in {code}");
        };
        let pages = |q: &str| -> anyhow::Result<Vec<Value>> {
            let mut out: Vec<Value> = vec![];
            let mut s = start;
            loop {
                let p = c.query(q, json!({ "code": code, "fid": [fight], "s": s, "e": end }))?;
                let page = &p["reportData"]["report"]["events"];
                out.extend(page["data"].as_array().into_iter().flatten().cloned());
                match page["nextPageTimestamp"].as_f64() {
                    Some(n) if n > s => s = n,
                    _ => break,
                }
            }
            Ok(out)
        };
        let deaths = pages(DEATHS_Q)?;
        let actors = &report["masterData"]["actors"];
        let mut run = super::ref_run(code, f, actors, &deaths)
            .ok_or_else(|| anyhow::anyhow!("{code}:{fight} is not a keystone run"))?;
        let npcs = actors
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["id"].as_i64())
            .collect();
        if let Some((n, damage)) =
            super::ref_damage(&report["playerDetails"], &npcs, &pages(DAMAGE_Q)?)
        {
            run.non_tanks = n;
            run.damage = damage;
        }
        Ok(run)
    }

    pub struct Fetched {
        pub title: String,
        pub events: Vec<Event>,
        /// keyed by fight start ts (matches FightReport.start_ts)
        pub rankings: HashMap<i64, HashMap<String, super::Rank>>,
        pub boss_pct: HashMap<i64, f64>,
    }

    pub fn fetch(
        c: &Client,
        code: &str,
        only: &[i64],
        with_rankings: bool,
    ) -> anyhow::Result<Fetched> {
        let meta = c.query(META_Q, json!({ "code": code }))?;
        let report = &meta["reportData"]["report"];
        if report.is_null() {
            bail!("report {code} not found (private reports need the user OAuth flow)");
        }
        let master = Master::from_json(&report["masterData"]);
        let fights: Vec<FightMeta> = report["fights"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(FightMeta::from_json)
            .filter(|f| {
                if only.is_empty() {
                    f.is_interesting()
                } else {
                    only.contains(&f.id)
                }
            })
            .collect();
        if fights.is_empty() {
            bail!("no boss or keystone fights selected in {code}");
        }
        let mut events = master.ownership_events(0);
        let mut rankings = HashMap::new();
        let mut boss_pct = HashMap::new();
        for f in &fights {
            eprintln!("  fetching fight {} ({})...", f.id, f.name);
            events.push(f.start_event());
            let mut fight_events = vec![];
            let mut s = f.start as f64;
            loop {
                let d = c.query(
                    EVENTS_Q,
                    json!({ "code": code, "fid": [f.id], "s": s, "e": f.end as f64 }),
                )?;
                let page = &d["reportData"]["report"]["events"];
                fight_events.extend(
                    page["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| master.convert(e)),
                );
                match page["nextPageTimestamp"].as_f64() {
                    Some(n) if n > s => s = n,
                    _ => break,
                }
            }
            events.extend(super::with_boss_events(fight_events, f.boss_events()));
            events.push(f.end_event());
            if let Some(p) = f.boss_pct.filter(|_| f.keystone_level.is_none()) {
                boss_pct.insert(f.start, if f.kill { 0.0 } else { p });
            }
            if with_rankings && f.encounter_id > 0 {
                match c.fight_rankings(code, f.id) {
                    Ok(r) => {
                        rankings.insert(f.start, r);
                    }
                    Err(e) => eprintln!("  (rankings unavailable for fight {}: {e})", f.id),
                }
            }
        }
        Ok(Fetched {
            title: report["title"].as_str().unwrap_or(code).to_string(),
            events,
            rankings,
            boss_pct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn master() -> Master {
        Master::from_json(&json!({
            "actors": [
                {"id": 1, "name": "Tanky", "type": "Player"},
                {"id": 2, "name": "Boss", "type": "NPC", "gameID": 424242},
                {"id": 3, "name": "Wolf", "type": "Pet", "petOwner": 1}
            ],
            "abilities": [{"gameID": 999, "name": "Big Slam", "type": "32"}]
        }))
    }

    #[test]
    fn converts_damage_and_heal() {
        let m = master();
        let e = m.convert(&json!({"timestamp": 500, "type": "damage", "sourceID": 2, "targetID": 1, "abilityGameID": 999, "amount": 100, "absorbed": 20, "sourceIsFriendly": false, "targetIsFriendly": true, "hitPoints": 50, "maxHitPoints": 100, "resourceActor": 2, "x": 38882, "y": 450407, "mapID": 2572})).unwrap();
        assert!(e.dst.as_ref().unwrap().is_group_player());
        assert!(e.src.as_ref().unwrap().is_hostile_npc());
        match e.kind {
            Kind::Damage {
                spell,
                amount,
                absorbed,
                info,
                ..
            } => {
                assert_eq!((spell.name.as_str(), spell.school), ("Big Slam", 32));
                assert_eq!((amount, absorbed), (100, 20));
                let info = info.unwrap();
                assert_eq!(info.max_hp, 100);
                // WCL hundredths of a yard, axes swapped, into the raw-log frame.
                assert_eq!(
                    info.pos,
                    Some(Pos {
                        x: 4504.07,
                        y: -388.82,
                        map: 2572
                    })
                );
            }
            k => panic!("{k:?}"),
        }
        let h = m.convert(&json!({"timestamp": 1, "type": "heal", "sourceID": 1, "targetID": 1, "abilityGameID": 999, "amount": 70, "overheal": 30})).unwrap();
        assert!(matches!(
            h.kind,
            Kind::Heal {
                amount: 100,
                overheal: 30,
                ..
            }
        ));
        assert_eq!(m.ownership_events(0).len(), 1);
    }

    #[test]
    fn npc_instances_are_separate_units() {
        // Two mobs of the same NPC type: one actor, two instances.
        let m = master();
        let a = m.convert(&json!({"timestamp": 1, "type": "begincast", "sourceID": 2, "sourceInstance": 1, "abilityGameID": 999})).unwrap();
        let b = m.convert(&json!({"timestamp": 2, "type": "begincast", "sourceID": 2, "sourceInstance": 2, "abilityGameID": 999})).unwrap();
        let c = m
            .convert(&json!({"timestamp": 3, "type": "death", "targetID": 2, "targetInstance": 2}))
            .unwrap();
        let guid = |e: &Event, src: bool| {
            (if src { &e.src } else { &e.dst })
                .as_ref()
                .unwrap()
                .guid
                .clone()
        };
        // The NPC ID sits where the game puts it in a Creature GUID.
        assert_eq!(guid(&a, true).split('-').nth(5), Some("424242"));
        assert_ne!(guid(&a, true), guid(&b, true));
        assert_eq!(
            guid(&b, true),
            guid(&c, false),
            "the death is the second mob's"
        );
        // No instance field means instance 1.
        let d = m
            .convert(
                &json!({"timestamp": 4, "type": "begincast", "sourceID": 2, "abilityGameID": 999}),
            )
            .unwrap();
        assert_eq!(guid(&a, true), guid(&d, true));
    }

    #[test]
    fn reference_run_from_pulls_and_enemy_deaths() {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/wcl-reference.json")).unwrap();
        let deaths: Vec<Value> = v["deaths"].as_array().unwrap().clone();
        let r = ref_run(
            "abc",
            &v["report"]["fights"][0],
            &v["report"]["masterData"]["actors"],
            &deaths,
        )
        .unwrap();
        assert_eq!(
            (r.map_id, r.level, r.time_ms, r.bonus, r.fight_id),
            (2813, 14, 1_135_000, Some(3), 7)
        );
        // Times from the run's start; actors mapped to NPC IDs; deaths of
        // unknown actors and other event types dropped.
        assert_eq!(
            r.kills,
            [(50_000, 234849), (51_000, 235257), (310_000, 234998)]
        );
        assert_eq!(
            r.pulls[1],
            crate::reference::RefPull {
                start_ms: 200_000,
                end_ms: 312_000,
                encounter_id: 3101,
                kill: true
            }
        );
        // Not a keystone run.
        let mut raid = v["report"]["fights"][0].clone();
        raid["keystoneLevel"] = Value::Null;
        assert!(ref_run("abc", &raid, &Value::Null, &[]).is_none());
    }

    #[test]
    fn ref_damage_counts_non_tank_hits_from_enemies() {
        let details = json!({ "data": { "playerDetails": {
            "tanks": [{ "id": 1 }], "healers": [{ "id": 2 }], "dps": [{ "id": 3 }, { "id": 4 }]
        }}});
        let npcs: std::collections::HashSet<i64> = [50].into();
        let hit = |src: i64, dst: i64, spell: i64, amount: i64, absorbed: i64| {
            json!({ "type": "damage", "sourceID": src, "targetID": dst,
                    "abilityGameID": spell, "amount": amount, "absorbed": absorbed })
        };
        let events = [
            hit(50, 3, 900, 100, 20),
            hit(50, 4, 900, 50, 0),
            hit(50, 1, 900, 999, 0), // the tank
            hit(3, 3, 32379, 10, 0), // a player's own Shadow Word: Death
            hit(-1, 2, 3, 30, 0),    // falling
            json!({ "type": "absorbed", "sourceID": 50, "targetID": 2, "abilityGameID": 900 }),
        ];
        let (n, d) = ref_damage(&details, &npcs, &events).unwrap();
        assert_eq!(n, 3);
        let got: Vec<(u32, u32, u64)> = d.iter().map(|d| (d.spell_id, d.hits, d.amount)).collect();
        assert_eq!(got, [(3, 1, 30), (900, 2, 170)]);
        assert!(ref_damage(&Value::Null, &npcs, &events).is_none());
    }

    #[test]
    fn keystone_fight_gets_map_id_and_boss_markers() {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/wcl-keystone-fight.json"))
                .unwrap();
        let f = FightMeta::from_json(&v["fights"][0]).unwrap();
        assert_eq!((f.game_zone, f.map_id()), (2923, 2923));
        // Trash pulls (encounterID 0) are not bosses.
        let bosses: Vec<(u32, &str)> = f
            .boss_pulls
            .iter()
            .map(|b| (b.encounter_id, b.name.as_str()))
            .collect();
        assert_eq!(
            bosses,
            [(3285, "Taz'Rah"), (3286, "Atroxus"), (3287, "Charonus")]
        );
        assert!(matches!(
            f.start_event().kind,
            Kind::ChallengeStart {
                map_id: 2923,
                level: 24,
                ..
            }
        ));
        assert!(matches!(
            f.end_event().kind,
            Kind::ChallengeEnd {
                map_id: 2923,
                success: true,
                ..
            }
        ));

        // Completed over time (bonus 0) is still completed: timed is judged
        // later, from the time against the pack's timer.
        let mut d = f.clone();
        (d.kill, d.keystone_bonus) = (true, Some(0));
        assert!(matches!(
            d.end_event().kind,
            Kind::ChallengeEnd { success: true, .. }
        ));
        (d.kill, d.keystone_bonus) = (false, None);
        assert!(matches!(
            d.end_event().kind,
            Kind::ChallengeEnd { success: false, .. }
        ));

        // Without gameZone, the map ID is decoded from the keystone encounter ID.
        let mut g = f.clone();
        g.game_zone = 0;
        assert_eq!(g.map_id(), 2923);

        // End to end: the analyzer sees a run with boss markers and their encounter IDs.
        let mut evs = vec![f.start_event()];
        let p = Unit {
            guid: "Player-1-1".into(),
            name: "P".into(),
            flags: TYPE_PLAYER | AFFILIATION_PARTY | REACTION_FRIENDLY,
        };
        let mob = Unit {
            guid: "Creature-0-1-1-1-1-1".into(),
            name: "M".into(),
            flags: TYPE_NPC | REACTION_HOSTILE,
        };
        let hits: Vec<Event> = (f.start..f.end)
            .step_by(60_000)
            .map(|ts| Event {
                ts,
                src: Some(p.clone()),
                dst: Some(mob.clone()),
                kind: Kind::Damage {
                    spell: Spell::melee(),
                    amount: 1,
                    overkill: 0,
                    absorbed: 0,
                    periodic: false,
                    info: None,
                },
            })
            .collect();
        evs.extend(with_boss_events(hits, f.boss_events()));
        evs.push(f.end_event());
        assert!(evs.windows(2).all(|w| w[0].ts <= w[1].ts), "time-ordered");
        let mut an = crate::analysis::Analyzer::new(crate::config::Config::default());
        for e in evs {
            an.feed(e);
        }
        let (fights, _) = an.finish();
        let crate::analysis::FightKind::MythicPlus { map_id, bosses, .. } = &fights[0].kind else {
            panic!("not M+")
        };
        assert_eq!(*map_id, 2923);
        let got: Vec<(u32, bool)> = bosses.iter().map(|b| (b.encounter_id, b.success)).collect();
        assert_eq!(got, [(3285, true), (3286, true), (3287, true)]);
    }

    #[test]
    fn fight_rankings_skip_anonymous_runs() {
        let v = json!({"hasMorePages": true, "rankings": [
            {"report": {"code": "", "fightID": null}, "bracketData": 24},
            {"report": {"code": "AbC", "fightID": 9}, "bracketData": 24},
            {"report": {"code": "XyZ", "fightID": 3}, "bracketData": 15}
        ]});
        let (keys, more) = parse_fight_rankings(&v);
        assert!(more);
        assert_eq!(
            keys,
            [
                RankedKey {
                    code: "AbC".into(),
                    fight_id: 9,
                    level: 24
                },
                RankedKey {
                    code: "XyZ".into(),
                    fight_id: 3,
                    level: 15
                },
            ]
        );
    }

    #[test]
    fn zone_encounters_skip_frozen_zones() {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/wcl-zones.json")).unwrap();
        let enc = zone_encounters(&v["data"]);
        assert!(enc.contains(&("Nek'zali the Soulcoiler".into(), 3470)));
        assert!(enc.contains(&("Nymrissa Wavecaller".into(), 3379)));
        assert!(enc.contains(&("Kith'ix".into(), 3513)));
        // M+ dungeons are listed as whole-run "encounters": ID = prefix * 10000 + map ID.
        assert!(enc.contains(&("Voidscar Arena".into(), 12923)));
        assert!(
            !enc.iter().any(|(n, _)| n == "Rotmire"),
            "PTR zones are skipped"
        );
        let frozen = json!({"worldData": {"expansions": [{"zones": [
            {"frozen": true, "encounters": [{"id": 1, "name": "Old"}]}
        ]}]}});
        assert!(zone_encounters(&frozen).is_empty());
    }

    #[test]
    fn rankings() {
        let fight = |t: f64, h: f64, d: f64| {
            json!({"data": [{"roles": {
                "tanks": {"characters": [{"name": "T", "rankPercent": t}]},
                "healers": {"characters": [{"name": "H", "rankPercent": h, "bracketPercent": h / 2.0}]},
                "dps": {"characters": [{"name": "A", "rankPercent": d, "bracketPercent": 9.0}]}
            }}]})
        };
        // DPS metric for tanks and DPS, HPS for healers.
        let r = parse_rankings(&fight(80.0, 10.0, 42.5), &fight(5.0, 84.0, 3.0));
        let o = |n: &str| (r[n].overall, r[n].bracket);
        assert_eq!(o("T"), (80.0, None));
        assert_eq!(o("H"), (84.0, Some(42.0)));
        assert_eq!(o("A"), (42.5, Some(9.0)));
    }

    #[test]
    fn players_realm_slugs_and_ranked_runs() {
        assert_eq!(
            split_player("Name-SteamwheedleCartel-US"),
            Some(("Name", "SteamwheedleCartel", "US"))
        );
        assert_eq!(split_player("Name"), None);
        assert_eq!(
            realm_slugs("SteamwheedleCartel"),
            ["steamwheedle-cartel", "steamwheedlecartel"]
        );
        assert_eq!(realm_slugs("Area52"), ["area-52", "area52"]);
        assert_eq!(realm_slugs("AzjolNerub"), ["azjol-nerub", "azjolnerub"]);
        assert_eq!(realm_slugs("Uldum"), ["uldum"]);
        assert_eq!(realm_slugs("Zul'jin"), ["zuljin"]);

        let v = json!({"ranks": [
            {"report": {"code": "old", "fightID": 1}, "startTime": 1_000_000, "bracketData": 15},
            {"report": {"code": "near", "fightID": 3}, "startTime": 9_100_000, "bracketData": 15},
            {"report": {"code": "lvl", "fightID": 2}, "startTime": 9_010_000, "bracketData": 14},
            {"report": {"code": "far", "fightID": 4}, "startTime": 9_000_000 + 400_000, "bracketData": 15}
        ]});
        let runs = ranked_runs(&v);
        assert_eq!(runs.len(), 4);
        let m = match_run(&runs, 9_000_000, 15).unwrap();
        assert_eq!((m.code.as_str(), m.fight_id), ("near", 3));
        assert!(match_run(&runs, 9_000_000, 16).is_none());
    }
}
