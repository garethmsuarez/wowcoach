//! Blizzard Game Data API → season pack (`wowcoach season sync`).
//!
//! What the API gives and doesn't is recorded in
//! `docs/milestones/m1-season-data.md`. In short: the tracked keystone
//! dungeons (map IDs, journal IDs, timers) and the Encounter Journal (bosses
//! and some of their spells). Not: logged encounter IDs, trash spells, or any
//! "interrupt this" flag. Those come from logs and reviewers.
//!
//! Layout: pure JSON mapping functions, [`fetch`] which walks the API through
//! the [`Api`] trait (so it's testable offline against saved responses),
//! [`merge`] into an existing pack without touching reviewed data, and
//! [`render`] to deterministic TOML. Network code is in [`net`].

use crate::season::{Boss, Dungeon, Raid, SeasonPack, Source, SpellEntry};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    /// Game data that changes with patches (journal, spells).
    Static,
    /// Data that changes during a patch (seasons, keystone dungeons).
    Dynamic,
}

/// One GET against the Game Data API, returning the JSON body.
pub trait Api {
    fn get(&self, path: &str, ns: Namespace) -> anyhow::Result<Value>;
}

// ----------------------------------------------------------------- mapping

fn u(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|x| u32::try_from(x).ok())
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn current_season_id(index: &Value) -> Option<u32> {
    u(&index["current_season"]["id"])
}

pub fn dungeon_ids(index: &Value) -> Vec<u32> {
    index["dungeons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| u(&d["id"]))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeystoneDungeon {
    pub keystone_id: u32,
    pub name: String,
    pub map_id: u32,
    pub journal_id: u32,
    /// Seconds for +1, +2, +3.
    pub timer_s: Vec<u32>,
    /// In the current season's dungeon pool.
    pub tracked: bool,
}

pub fn keystone_dungeon(v: &Value) -> Option<KeystoneDungeon> {
    let mut ups: Vec<(u64, u32)> = v["keystone_upgrades"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            Some((
                x["upgrade_level"].as_u64()?,
                (x["qualifying_duration"].as_u64()? / 1000) as u32,
            ))
        })
        .collect();
    ups.sort();
    Some(KeystoneDungeon {
        keystone_id: u(&v["id"])?,
        name: s(&v["name"]),
        map_id: u(&v["map"]["id"])?,
        journal_id: u(&v["dungeon"]["id"]).unwrap_or(0),
        timer_s: ups.into_iter().map(|x| x.1).collect(),
        tracked: v["is_tracked"].as_bool().unwrap_or(false),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct JournalInstance {
    pub journal_id: u32,
    pub name: String,
    pub map_id: u32,
    /// (journal encounter ID, name), in journal order.
    pub encounters: Vec<(u32, String)>,
}

pub fn journal_instance(v: &Value) -> Option<JournalInstance> {
    Some(JournalInstance {
        journal_id: u(&v["id"])?,
        name: s(&v["name"]),
        map_id: u(&v["map"]["id"]).unwrap_or(0),
        encounters: v["encounters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| Some((u(&e["id"])?, s(&e["name"]))))
            .collect(),
    })
}

/// Every spell named in a journal encounter's (nested) sections, deduplicated,
/// in journal order.
pub fn encounter_spells(v: &Value) -> Vec<(u32, String)> {
    fn walk(secs: &Value, seen: &mut BTreeSet<u32>, out: &mut Vec<(u32, String)>) {
        for sec in secs.as_array().into_iter().flatten() {
            if let Some(id) = u(&sec["spell"]["id"])
                && seen.insert(id)
            {
                let name = sec["spell"]["name"].as_str().or(sec["title"].as_str());
                out.push((id, name.unwrap_or_default().to_string()));
            }
            walk(&sec["sections"], seen, out);
        }
    }
    let mut out = vec![];
    walk(&v["sections"], &mut BTreeSet::new(), &mut out);
    out
}

/// "12.1.5" from any `namespace=static-12.1.5_70077-us` link in a response.
pub fn build_version(v: &Value) -> Option<String> {
    let text = v.to_string();
    let i = text.find("namespace=static-")? + "namespace=static-".len();
    let rest = &text[i..];
    let end = rest.find(['_', '-', '"', '&'])?;
    let b = &rest[..end];
    (!b.is_empty() && b.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| b.to_string())
}

// ------------------------------------------------------------------- fetch

/// A boss and the spells its journal entry names.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncedBoss {
    pub journal_id: u32,
    pub name: String,
    pub spells: Vec<(u32, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyncedInstance {
    pub journal: JournalInstance,
    pub bosses: Vec<SyncedBoss>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Synced {
    pub season_id: u32,
    pub season_name: String,
    pub build: Option<String>,
    /// The current keystone pool with each dungeon's journal data.
    pub dungeons: Vec<(KeystoneDungeon, SyncedInstance)>,
    pub raids: Vec<SyncedInstance>,
}

fn instance(
    api: &dyn Api,
    journal_id: u32,
    build: &mut Option<String>,
) -> anyhow::Result<SyncedInstance> {
    let v = api.get(
        &format!("/data/wow/journal-instance/{journal_id}"),
        Namespace::Static,
    )?;
    if build.is_none() {
        *build = build_version(&v);
    }
    let journal = journal_instance(&v)
        .ok_or_else(|| anyhow::anyhow!("journal instance {journal_id}: unexpected response"))?;
    let mut bosses = vec![];
    for (id, name) in &journal.encounters {
        let e = api.get(
            &format!("/data/wow/journal-encounter/{id}"),
            Namespace::Static,
        )?;
        bosses.push(SyncedBoss {
            journal_id: *id,
            name: name.clone(),
            spells: encounter_spells(&e),
        });
    }
    Ok(SyncedInstance { journal, bosses })
}

/// Walk the API: current season, its tracked dungeons and their journal
/// entries, plus the given raids (journal instance IDs). `progress` gets one
/// line per instance.
pub fn fetch(
    api: &dyn Api,
    raid_journal_ids: &[u32],
    mut progress: impl FnMut(&str),
) -> anyhow::Result<Synced> {
    let index = api.get("/data/wow/mythic-keystone/season/index", Namespace::Dynamic)?;
    let season_id = current_season_id(&index)
        .ok_or_else(|| anyhow::anyhow!("no current M+ season in the API"))?;
    let season = api.get(
        &format!("/data/wow/mythic-keystone/season/{season_id}"),
        Namespace::Dynamic,
    )?;
    let season_name = season["season_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The season itself doesn't list its dungeons: every keystone dungeon
    // ever made is in the index, and `is_tracked` marks the current pool.
    let ids = dungeon_ids(&api.get(
        "/data/wow/mythic-keystone/dungeon/index",
        Namespace::Dynamic,
    )?);
    let mut tracked = vec![];
    for id in ids {
        let d = api.get(
            &format!("/data/wow/mythic-keystone/dungeon/{id}"),
            Namespace::Dynamic,
        )?;
        if let Some(k) = keystone_dungeon(&d).filter(|k| k.tracked) {
            tracked.push(k);
        }
    }
    if tracked.is_empty() {
        anyhow::bail!(
            "the API lists no tracked keystone dungeons; refusing to sync an empty dungeon pool"
        );
    }

    let mut build = None;
    let mut dungeons = vec![];
    for k in tracked {
        progress(&format!("dungeon {} (journal {})", k.name, k.journal_id));
        let inst = instance(api, k.journal_id, &mut build)?;
        dungeons.push((k, inst));
    }
    let mut raids = vec![];
    for id in raid_journal_ids {
        let inst = instance(api, *id, &mut build)?;
        progress(&format!("raid {} (journal {id})", inst.journal.name));
        raids.push(inst);
    }
    Ok(Synced {
        season_id,
        season_name,
        build,
        dungeons,
        raids,
    })
}

// ------------------------------------------------------------------- merge

/// What a merge changed, for the console.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeSummary {
    pub dungeons_added: Vec<String>,
    pub dungeons_removed: Vec<String>,
    pub raids_added: Vec<String>,
    pub bosses_added: usize,
    pub spells_added: usize,
    /// Journal IDs added to an existing entry of the same name.
    pub ids_added: usize,
    /// Spell entries dropped with a dungeon that left the pool.
    pub spells_removed: usize,
    pub encounter_ids_filled: usize,
    /// Boss names a `wcl_encounters` list didn't match.
    pub bosses_without_encounter_id: Vec<String>,
    /// (boss, pack ID, WCL ID) where they disagree; the pack is left alone.
    pub encounter_id_conflicts: Vec<(String, u32, u32)>,
}

impl MergeSummary {
    pub fn is_noop(&self) -> bool {
        self.dungeons_added.is_empty()
            && self.dungeons_removed.is_empty()
            && self.raids_added.is_empty()
            && self.bosses_added == 0
            && self.spells_added == 0
            && self.ids_added == 0
            && self.spells_removed == 0
            && self.encounter_ids_filled == 0
    }
}

impl std::fmt::Display for MergeSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_noop() {
            writeln!(f, "No changes.")?;
        }
        if !self.dungeons_added.is_empty() {
            writeln!(f, "Dungeons added: {}", self.dungeons_added.join(", "))?;
        }
        if !self.dungeons_removed.is_empty() {
            writeln!(
                f,
                "Dungeons removed (left the pool): {} ({} spell entries dropped with them)",
                self.dungeons_removed.join(", "),
                self.spells_removed
            )?;
        }
        if !self.raids_added.is_empty() {
            writeln!(f, "Raids added: {}", self.raids_added.join(", "))?;
        }
        if self.bosses_added + self.spells_added + self.ids_added + self.encounter_ids_filled > 0 {
            writeln!(
                f,
                "Bosses added: {}. Spells added: {}. Journal IDs added to existing spells: {}. Encounter IDs filled: {}.",
                self.bosses_added, self.spells_added, self.ids_added, self.encounter_ids_filled
            )?;
        }
        for (b, ours, theirs) in &self.encounter_id_conflicts {
            writeln!(
                f,
                "Warning: {b} has encounter_id {ours} in the pack but {theirs} on Warcraft Logs; kept {ours}."
            )?;
        }
        if !self.bosses_without_encounter_id.is_empty() {
            writeln!(
                f,
                "Still without an encounter ID (filled from logs by `season suggest`): {}",
                self.bosses_without_encounter_id.join(", ")
            )?;
        }
        Ok(())
    }
}

/// Journal order, keeping what the pack already knows (logged encounter IDs).
fn merge_bosses(old: &[Boss], synced: &[SyncedBoss], summary: &mut MergeSummary) -> Vec<Boss> {
    let mut out: Vec<Boss> = synced
        .iter()
        .map(|b| {
            let prev = old
                .iter()
                .find(|o| (o.journal_id != 0 && o.journal_id == b.journal_id) || o.name == b.name);
            if prev.is_none() {
                summary.bosses_added += 1;
            }
            Boss {
                name: b.name.clone(),
                journal_id: b.journal_id,
                encounter_id: prev.map(|p| p.encounter_id).unwrap_or(0),
            }
        })
        .collect();
    // Bosses the journal no longer lists but a reviewer/log added: keep them.
    for o in old {
        if !out.iter().any(|b| b.name == o.name) {
            out.push(o.clone());
        }
    }
    out
}

fn merge_spells(
    pack: &mut SeasonPack,
    map_id: u32,
    bosses: &[SyncedBoss],
    summary: &mut MergeSummary,
) {
    for b in bosses {
        for (id, name) in &b.spells {
            if let Some(e) = pack.spells.iter_mut().find(|e| e.ids.contains(id)) {
                if !e.sources.contains(&Source::Journal) {
                    e.sources.push(Source::Journal);
                }
                continue;
            }
            // Same mechanic already known under other IDs (logged, or another
            // journal section of this boss): add the journal ID to it.
            if let Some(e) = pack.spells.iter_mut().find(|e| {
                e.map_id == map_id
                        && !name.is_empty()
                        && e.name == *name
                        // Same name on another boss is usually another mechanic.
                        && e.boss.as_ref().is_none_or(|eb| *eb == b.name)
            }) {
                e.ids.push(*id);
                if !e.sources.contains(&Source::Journal) {
                    e.sources.push(Source::Journal);
                }
                summary.ids_added += 1;
                continue;
            }
            pack.spells.push(SpellEntry {
                ids: vec![*id],
                name: name.clone(),
                map_id,
                boss: Some(b.name.clone()),
                sources: vec![Source::Journal],
                ..Default::default()
            });
            summary.spells_added += 1;
        }
    }
}

/// Merge synced data into `pack`. Never touches `tags`, `suggested`, `note`,
/// spell IDs learned from logs, or known encounter IDs. Dungeons that left the
/// keystone pool are removed with their spells. `wcl_encounters` (boss name →
/// logged encounter ID, from Warcraft Logs zone data) fills missing encounter IDs.
pub fn merge(
    pack: &mut SeasonPack,
    synced: &Synced,
    wcl_encounters: &[(String, u32)],
) -> MergeSummary {
    let mut sum = MergeSummary::default();
    pack.season.blizzard_season_id = Some(synced.season_id);
    if !synced.season_name.is_empty() {
        pack.season.name.clone_from(&synced.season_name);
    }
    if synced.build.is_some() {
        pack.season.synced_build.clone_from(&synced.build);
    }

    // Dungeons: exactly the current pool.
    let pool: BTreeSet<u32> = synced.dungeons.iter().map(|(k, _)| k.map_id).collect();
    for d in pack.dungeons.iter().filter(|d| !pool.contains(&d.map_id)) {
        sum.dungeons_removed.push(d.name.clone());
        let before = pack.spells.len();
        pack.spells.retain(|s| s.map_id != d.map_id);
        sum.spells_removed += before - pack.spells.len();
    }
    pack.dungeons.retain(|d| pool.contains(&d.map_id));
    for (k, inst) in &synced.dungeons {
        let idx = match pack.dungeons.iter().position(|d| d.map_id == k.map_id) {
            Some(i) => i,
            None => {
                sum.dungeons_added.push(k.name.clone());
                pack.dungeons.push(Dungeon::default());
                pack.dungeons.len() - 1
            }
        };
        let d = &mut pack.dungeons[idx];
        d.name.clone_from(&k.name);
        d.keystone_id = k.keystone_id;
        d.map_id = k.map_id;
        d.journal_id = k.journal_id;
        d.timer_s.clone_from(&k.timer_s);
        d.bosses = merge_bosses(&d.bosses, &inst.bosses, &mut sum);
        merge_spells(pack, k.map_id, &inst.bosses, &mut sum);
    }

    // Raids: add or update the requested ones; others already in the pack stay.
    for inst in &synced.raids {
        let j = &inst.journal;
        let idx = match pack
            .raids
            .iter()
            .position(|r| r.journal_id == j.journal_id || r.map_id == j.map_id)
        {
            Some(i) => i,
            None => {
                sum.raids_added.push(j.name.clone());
                pack.raids.push(Raid::default());
                pack.raids.len() - 1
            }
        };
        let r = &mut pack.raids[idx];
        r.name.clone_from(&j.name);
        r.map_id = j.map_id;
        r.journal_id = j.journal_id;
        r.bosses = merge_bosses(&r.bosses, &inst.bosses, &mut sum);
        merge_spells(pack, j.map_id, &inst.bosses, &mut sum);
    }

    // Logged encounter IDs from Warcraft Logs, by boss name (raids; WCL zone
    // data doesn't list M+ bosses, which `season suggest` fills from logs).
    if !wcl_encounters.is_empty() {
        let mut by_name: HashMap<String, u32> = HashMap::new();
        for (n, id) in wcl_encounters {
            by_name.entry(n.to_lowercase()).or_insert(*id);
        }
        for r in pack.raids.iter_mut() {
            for b in r.bosses.iter_mut() {
                match by_name.get(&b.name.to_lowercase()) {
                    Some(&id) if b.encounter_id == 0 => {
                        b.encounter_id = id;
                        sum.encounter_ids_filled += 1;
                    }
                    Some(&id) if b.encounter_id != id => {
                        sum.encounter_id_conflicts
                            .push((b.name.clone(), b.encounter_id, id));
                    }
                    Some(_) => {}
                    None if b.encounter_id == 0 => {
                        sum.bosses_without_encounter_id.push(b.name.clone())
                    }
                    None => {}
                }
            }
        }
    }
    sort(pack);
    sum
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

// ----------------------------------------------------------------- network

pub mod net {
    use super::{Api, Namespace};
    use anyhow::Context;
    use serde_json::Value;

    pub const TOKEN_URL: &str = "https://oauth.battle.net/token";

    pub struct Client {
        agent: ureq::Agent,
        token: String,
        region: String,
    }

    impl Client {
        /// `region`: us, eu, kr or tw.
        pub fn connect(id: &str, secret: &str, region: &str) -> anyhow::Result<Client> {
            let region = region.to_lowercase();
            if !["us", "eu", "kr", "tw"].contains(&region.as_str()) {
                anyhow::bail!("unknown region {region:?} (expected us, eu, kr or tw)");
            }
            let agent = crate::net::agent();
            let token =
                crate::net::client_credentials(&agent, TOKEN_URL, id, secret, "Battle.net")?;
            Ok(Client {
                agent,
                token,
                region,
            })
        }
    }

    impl Api for Client {
        fn get(&self, path: &str, ns: Namespace) -> anyhow::Result<Value> {
            let ns = match ns {
                Namespace::Static => format!("static-{}", self.region),
                Namespace::Dynamic => format!("dynamic-{}", self.region),
            };
            let url = format!("https://{}.api.blizzard.com{path}", self.region);
            self.agent
                .get(&url)
                .set("Authorization", &format!("Bearer {}", self.token))
                .set("Battlenet-Namespace", &ns)
                .query("locale", "en_US")
                .call()
                .with_context(|| format!("GET {path}"))?
                .into_json()
                .with_context(|| format!("decoding {path}"))
        }
    }
}

#[cfg(test)]
mod tests;
