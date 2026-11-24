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
}

#[derive(Default)]
pub struct Master {
    pub actors: HashMap<i64, Actor>,
    pub abilities: HashMap<i64, String>,
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
                    },
                );
            }
        }
        for a in md["abilities"].as_array().into_iter().flatten() {
            if let Some(id) = i(a, "gameID") {
                m.abilities
                    .insert(id, a["name"].as_str().unwrap_or("?").to_string());
            }
        }
        m
    }

    fn guid(&self, id: i64) -> String {
        match self.actors.get(&id).map(|a| a.kind.as_str()) {
            Some("Player") => format!("Player-WCL-{id}"),
            Some("Pet") => format!("Pet-WCL-{id}"),
            _ => format!("Creature-WCL-{id}"),
        }
    }

    fn unit(&self, id: Option<i64>, friendly: Option<bool>) -> Option<Unit> {
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
            guid: self.guid(id),
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
        Spell {
            id: id.max(0) as u32,
            name: self
                .abilities
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Spell {id}")),
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
                        },
                    },
                })
            })
            .collect()
    }

    /// Convert one WCL event object into our model.
    pub fn convert(&self, e: &Value) -> Option<Event> {
        let ts = i(e, "timestamp")?;
        let src = self.unit(
            i(e, "sourceID"),
            e.get("sourceIsFriendly").and_then(Value::as_bool),
        );
        let dst = self.unit(
            i(e, "targetID"),
            e.get("targetIsFriendly").and_then(Value::as_bool),
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
            Some(AdvInfo {
                guid: who.as_ref()?.guid.clone(),
                owner_guid: "0000000000000000".into(),
                hp: u(e, "hitPoints"),
                max_hp: max,
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
        })
    }

    pub fn is_interesting(&self) -> bool {
        self.encounter_id > 0 || self.keystone_level.is_some()
    }

    pub fn start_event(&self) -> Event {
        let kind = match self.keystone_level {
            Some(level) => Kind::ChallengeStart {
                zone: self.name.clone(),
                map_id: 0,
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
                map_id: 0,
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
    use anyhow::{Context, bail};
    use serde_json::json;

    fn b64(input: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in input.chunks(3) {
            let n = (c[0] as u32) << 16
                | (*c.get(1).unwrap_or(&0) as u32) << 8
                | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= c.len() {
                    s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    pub struct Client {
        agent: ureq::Agent,
        token: String,
    }

    impl Client {
        pub fn connect(id: &str, secret: &str) -> anyhow::Result<Client> {
            let agent = ureq::AgentBuilder::new().try_proxy_from_env(true).build();
            let resp: Value = agent
                .post(TOKEN_URL)
                .set(
                    "Authorization",
                    &format!("Basic {}", b64(format!("{id}:{secret}").as_bytes())),
                )
                .send_form(&[("grant_type", "client_credentials")])
                .context("requesting Warcraft Logs access token")?
                .into_json()?;
            let token = resp["access_token"]
                .as_str()
                .context("no access_token in response")?
                .to_string();
            Ok(Client { agent, token })
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
        masterData{ actors{ id name type subType petOwner } abilities{ gameID name } }
        fights{ id encounterID name startTime endTime kill bossPercentage difficulty size keystoneLevel keystoneTime keystoneBonus keystoneAffixes }
    }}}"#;

    const EVENTS_Q: &str = r#"query($code:String!,$fid:[Int]!,$s:Float!,$e:Float!){ reportData{ report(code:$code){
        events(fightIDs:$fid,startTime:$s,endTime:$e,limit:10000,includeResources:true){ data nextPageTimestamp }
    }}}"#;

    const RANK_Q: &str = r#"query($code:String!,$fid:[Int]!){ reportData{ report(code:$code){ rankings(fightIDs:$fid) } } }"#;

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
            let mut s = f.start as f64;
            loop {
                let d = c.query(
                    EVENTS_Q,
                    json!({ "code": code, "fid": [f.id], "s": s, "e": f.end as f64 }),
                )?;
                let page = &d["reportData"]["report"]["events"];
                events.extend(
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
                {"id": 2, "name": "Boss", "type": "NPC"},
                {"id": 3, "name": "Wolf", "type": "Pet", "petOwner": 1}
            ],
            "abilities": [{"gameID": 999, "name": "Big Slam"}]
        }))
    }

    #[test]
    fn converts_damage_and_heal() {
        let m = master();
        let e = m.convert(&json!({"timestamp": 500, "type": "damage", "sourceID": 2, "targetID": 1, "abilityGameID": 999, "amount": 100, "absorbed": 20, "sourceIsFriendly": false, "targetIsFriendly": true, "hitPoints": 50, "maxHitPoints": 100, "resourceActor": 2})).unwrap();
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
                assert_eq!(spell.name, "Big Slam");
                assert_eq!((amount, absorbed), (100, 20));
                assert_eq!(info.unwrap().max_hp, 100);
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
    fn rankings() {
        let r = parse_rankings(
            &json!({"data": [{"roles": {"dps": {"characters": [{"name": "A", "rankPercent": 42.5}]}}}]}),
        );
        assert_eq!(r["A"], 42.5);
    }
}
