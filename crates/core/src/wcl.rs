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
                success: self.keystone_bonus.map(|b| b > 0).unwrap_or(self.kill),
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

/// name -> rankPercent from a `report.rankings` JSON blob.
pub fn parse_rankings(v: &Value) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for fight in v["data"].as_array().into_iter().flatten() {
        for role in ["tanks", "healers", "dps"] {
            for c in fight["roles"][role]["characters"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let (Some(n), Some(r)) = (c["name"].as_str(), c["rankPercent"].as_f64()) {
                    out.insert(n.to_string(), r);
                }
            }
        }
    }
    out
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

    const RANK_Q: &str = r#"query($code:String!,$fid:[Int]!){ reportData{ report(code:$code){ rankings(fightIDs:$fid) } } }"#;

    const RATE_Q: &str = r#"{ rateLimitData{ limitPerHour pointsSpentThisHour pointsResetIn } }"#;

    const TOP_Q: &str = r#"query($id:Int!,$page:Int!){ worldData{ encounter(id:$id){ fightRankings(page:$page) } } }"#;

    const TOP_BRACKET_Q: &str = r#"query($id:Int!,$page:Int!,$bracket:Int!){ worldData{ encounter(id:$id){ fightRankings(page:$page, bracket:$bracket) } } }"#;

    const ZONES_Q: &str =
        r#"{ worldData{ expansions{ id name zones{ id name frozen encounters{ id name } } } } }"#;

    impl Client {
        /// Boss name -> logged encounter ID for current content.
        pub fn zone_encounters(&self) -> anyhow::Result<Vec<(String, u32)>> {
            Ok(super::zone_encounters(&self.query(ZONES_Q, json!({}))?))
        }
    }

    pub struct Fetched {
        pub title: String,
        pub events: Vec<Event>,
        /// keyed by fight start ts (matches FightReport.start_ts)
        pub rankings: HashMap<i64, HashMap<String, f64>>,
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
                match c.query(RANK_Q, json!({ "code": code, "fid": [f.id] })) {
                    Ok(r) => {
                        rankings.insert(
                            f.start,
                            parse_rankings(&r["reportData"]["report"]["rankings"]),
                        );
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
        let r = parse_rankings(
            &json!({"data": [{"roles": {"dps": {"characters": [{"name": "A", "rankPercent": 42.5}]}}}]}),
        );
        assert_eq!(r["A"], 42.5);
    }
}
