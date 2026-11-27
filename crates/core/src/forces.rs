//! Enemy forces: how much each NPC counts toward a key's 100%.
//!
//! Neither the combat log nor the Blizzard API has it, so `season forces
//! --mdt DIR` imports it from a local Mythic Dungeon Tools checkout (the
//! addon's `Midnight/*.lua` data, GPL-2.0) into the pack's `[[npc]]` table,
//! and `season forces LOGS` checks the table against completed keys: a
//! completed key reached 100%, so kills that sum to less mean an NPC is
//! logged under an ID the table doesn't know.
//!
//! A kill is a `UNIT_DIED`, or a unit last seen at 1% health or less that
//! never logs a death: some are scripted away at 1 HP (Kings' Rest's Shadow of
//! Zul) and still count.

use crate::model::{Event, Kind, Unit};
use crate::season::{Npc, SeasonPack, Source};
use std::collections::{BTreeMap, BTreeSet, HashSet};

// ------------------------------------------------------------------ Lua

/// The subset of Lua values MDT's data files use. Expressions we don't need
/// (function calls, string concatenation) become `Other`.
#[derive(Clone, Debug, PartialEq)]
enum Lua {
    Str(String),
    Num(f64),
    Bool(bool),
    Nil,
    Table(Vec<(Key, Lua)>),
    Other,
}

#[derive(Clone, Debug, PartialEq)]
enum Key {
    Str(String),
    Num(f64),
    Pos(usize),
}

impl Lua {
    fn get(&self, k: &str) -> Option<&Lua> {
        match self {
            Lua::Table(t) => t
                .iter()
                .find(|(key, _)| matches!(key, Key::Str(s) if s == k))
                .map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Lua::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            Lua::Str(s) => Some(s),
            _ => None,
        }
    }
    fn values(&self) -> impl Iterator<Item = &Lua> {
        let t: &[(Key, Lua)] = match self {
            Lua::Table(t) => t,
            _ => &[],
        };
        t.iter().map(|(_, v)| v)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Str(String),
    Num(f64),
    Name(String),
    Sym(&'static str),
}

fn tokens(src: &str) -> anyhow::Result<Vec<Tok>> {
    let c: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '-' && c.get(i + 1) == Some(&'-') {
            // Comment: `--[[ ... ]]` or to the end of the line.
            if c.get(i + 2) == Some(&'[') && c.get(i + 3) == Some(&'[') {
                i += 4;
                while i + 1 < c.len() && !(c[i] == ']' && c[i + 1] == ']') {
                    i += 1;
                }
                i += 2;
            } else {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
        } else if ch == '"' || ch == '\'' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != ch {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                    s.push(match c[i] {
                        'n' => '\n',
                        't' => '\t',
                        x => x,
                    });
                } else {
                    s.push(c[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if ch.is_ascii_digit()
            || (ch == '-'
                && c.get(i + 1)
                    .is_some_and(|d| d.is_ascii_digit() || *d == '.'))
            || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            i += 1;
            while i < c.len()
                && (c[i].is_ascii_alphanumeric()
                    || c[i] == '.'
                    || ((c[i] == '-' || c[i] == '+') && matches!(c[i - 1], 'e' | 'E')))
            {
                i += 1;
            }
            let t: String = c[start..i].iter().collect();
            let n = t
                .parse::<f64>()
                .or_else(|_| {
                    let neg = t.starts_with('-');
                    let h = t.trim_start_matches('-').trim_start_matches("0x");
                    i64::from_str_radix(h, 16).map(|v| if neg { -v } else { v } as f64)
                })
                .map_err(|_| anyhow::anyhow!("bad number {t:?}"))?;
            out.push(Tok::Num(n));
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(c[start..i].iter().collect()));
        } else {
            let sym = match (ch, c.get(i + 1)) {
                ('.', Some('.')) => "..",
                ('=', Some('=')) => "==",
                _ => match ch {
                    '{' => "{",
                    '}' => "}",
                    '[' => "[",
                    ']' => "]",
                    '(' => "(",
                    ')' => ")",
                    '=' => "=",
                    ',' => ",",
                    ';' => ";",
                    '.' => ".",
                    ':' => ":",
                    '#' => "#",
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '<' => "<",
                    '>' => ">",
                    _ => anyhow::bail!("unexpected character {ch:?}"),
                },
            };
            i += sym.len();
            out.push(Tok::Sym(sym));
        }
    }
    Ok(out)
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn is(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }
    fn eat(&mut self, s: &str) -> anyhow::Result<()> {
        if self.is(s) {
            self.i += 1;
            Ok(())
        } else {
            anyhow::bail!("expected {s:?}, found {:?}", self.peek())
        }
    }

    /// A value, followed by any number of `.. value` (concatenation).
    fn value(&mut self) -> anyhow::Result<Lua> {
        let v = self.simple()?;
        if !self.is("..") {
            return Ok(v);
        }
        while self.is("..") {
            self.i += 1;
            self.simple()?;
        }
        Ok(Lua::Other)
    }

    fn simple(&mut self) -> anyhow::Result<Lua> {
        let Some(t) = self.peek().cloned() else {
            anyhow::bail!("unexpected end of input");
        };
        self.i += 1;
        Ok(match t {
            Tok::Str(s) => Lua::Str(s),
            Tok::Num(n) => Lua::Num(n),
            Tok::Sym("{") => self.table()?,
            Tok::Sym("-") => match self.simple()? {
                Lua::Num(n) => Lua::Num(-n),
                _ => Lua::Other,
            },
            Tok::Name(n) => match n.as_str() {
                "true" => Lua::Bool(true),
                "false" => Lua::Bool(false),
                "nil" => Lua::Nil,
                _ => {
                    // A name with field, index and call suffixes. `L["Key"]`
                    // (MDT's locale lookup) reads as its key.
                    let mut out = Lua::Other;
                    loop {
                        if self.is(".") || self.is(":") {
                            self.i += 2;
                        } else if self.is("[") {
                            self.i += 1;
                            let k = self.value()?;
                            self.eat("]")?;
                            out = match (&n[..], k) {
                                ("L", Lua::Str(s)) => Lua::Str(s),
                                _ => Lua::Other,
                            };
                        } else if self.is("(") {
                            self.i += 1;
                            while !self.is(")") {
                                self.value()?;
                                if self.is(",") {
                                    self.i += 1;
                                }
                            }
                            self.i += 1;
                            out = Lua::Other;
                        } else {
                            break;
                        }
                    }
                    out
                }
            },
            t => anyhow::bail!("unexpected {t:?}"),
        })
    }

    fn table(&mut self) -> anyhow::Result<Lua> {
        let mut out = vec![];
        let mut pos = 0;
        while !self.is("}") {
            if self.is("[") {
                self.i += 1;
                let k = match self.value()? {
                    Lua::Str(s) => Key::Str(s),
                    Lua::Num(n) => Key::Num(n),
                    _ => anyhow::bail!("unsupported table key"),
                };
                self.eat("]")?;
                self.eat("=")?;
                out.push((k, self.value()?));
            } else if let (Some(Tok::Name(n)), Some(Tok::Sym("="))) =
                (self.peek().cloned(), self.t.get(self.i + 1))
            {
                self.i += 2;
                out.push((Key::Str(n), self.value()?));
            } else {
                pos += 1;
                out.push((Key::Pos(pos), self.value()?));
            }
            if self.is(",") || self.is(";") {
                self.i += 1;
            } else if !self.is("}") {
                anyhow::bail!("expected ',' or '}}', found {:?}", self.peek());
            }
        }
        self.i += 1;
        Ok(Lua::Table(out))
    }
}

/// The value assigned by the first `target = value` statement in `src`.
fn assigned(src: &str, target: &str) -> anyhow::Result<Option<Lua>> {
    let Some(at) = src.find(&format!("{target} =")) else {
        return Ok(None);
    };
    let rest = &src[at + target.len()..];
    let t = tokens(rest)?;
    let mut p = P { t: &t, i: 0 };
    p.eat("=")?;
    Ok(Some(p.value()?))
}

// ------------------------------------------------------------------ MDT

/// One dungeon from an MDT data file.
#[derive(Clone, Debug, PartialEq)]
pub struct MdtDungeon {
    /// `mapInfo.mapID`: the keystone (challenge mode) ID.
    pub keystone_id: u32,
    pub name: String,
    pub total: u32,
    pub enemies: Vec<MdtEnemy>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MdtEnemy {
    pub id: u32,
    pub name: String,
    pub count: u32,
    pub is_boss: bool,
}

/// Parse one MDT dungeon file. Files without enemy data give `Ok(None)`.
pub fn parse_mdt(src: &str) -> anyhow::Result<Option<MdtDungeon>> {
    let (Some(info), Some(total), Some(enemies)) = (
        assigned(src, "MDT.mapInfo[dungeonIndex]")?,
        assigned(src, "MDT.dungeonTotalCount[dungeonIndex]")?,
        assigned(src, "MDT.dungeonEnemies[dungeonIndex]")?,
    ) else {
        return Ok(None);
    };
    let keystone_id = info
        .get("mapID")
        .and_then(Lua::num)
        .ok_or_else(|| anyhow::anyhow!("mapInfo has no mapID"))? as u32;
    let name = info
        .get("englishName")
        .and_then(Lua::str)
        .unwrap_or_default()
        .to_string();
    let total = total
        .get("normal")
        .and_then(Lua::num)
        .ok_or_else(|| anyhow::anyhow!("dungeonTotalCount has no normal"))? as u32;
    let enemies = enemies
        .values()
        .filter_map(|e| {
            Some(MdtEnemy {
                id: e.get("id")?.num()? as u32,
                name: e
                    .get("name")
                    .and_then(Lua::str)
                    .unwrap_or_default()
                    .to_string(),
                count: e.get("count").and_then(Lua::num).unwrap_or(0.0) as u32,
                is_boss: e.get("isBoss") == Some(&Lua::Bool(true)),
            })
        })
        .collect();
    Ok(Some(MdtDungeon {
        keystone_id,
        name,
        total,
        enemies,
    }))
}

/// Every MDT dungeon file in `dir` or `dir/Midnight`.
pub fn load_mdt(dir: &std::path::Path) -> anyhow::Result<Vec<MdtDungeon>> {
    let sub = dir.join("Midnight");
    let dir = if sub.is_dir() { sub } else { dir.to_path_buf() };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    files.sort();
    let mut out = vec![];
    for f in files {
        let src = std::fs::read_to_string(&f)?;
        if let Some(d) = parse_mdt(&src).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))? {
            out.push(d);
        }
    }
    Ok(out)
}

/// What `import` changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    /// (dungeon, forces total, NPCs worth forces).
    pub dungeons: Vec<(String, u32, usize)>,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Pack dungeons MDT has no data for.
    pub missing: Vec<String>,
}

/// Write MDT's forces into the pack. Only `source = "mdt"` entries are
/// added, changed or removed; an ID a reviewer entered by hand (or `--check`
/// added) is left as it is.
pub fn import(pack: &mut SeasonPack, mdt: &[MdtDungeon]) -> Imported {
    let mut out = Imported::default();
    for d in pack.dungeons.iter_mut() {
        let Some(m) = mdt.iter().find(|m| m.keystone_id == d.keystone_id) else {
            out.missing.push(d.name.clone());
            continue;
        };
        d.forces_total = m.total;
        // One NPC can be listed twice (different spawns): keep the higher count.
        let mut want: BTreeMap<u32, (&str, u32)> = BTreeMap::new();
        for e in m.enemies.iter().filter(|e| e.count > 0) {
            let w = want.entry(e.id).or_insert((&e.name, 0));
            w.1 = w.1.max(e.count);
        }
        let map = d.map_id;
        let before = pack.npcs.len();
        pack.npcs
            .retain(|n| n.map_id != map || n.source != Source::Mdt || want.contains_key(&n.id));
        out.removed += before - pack.npcs.len();
        for (id, (name, count)) in &want {
            match pack
                .npcs
                .iter_mut()
                .find(|n| n.map_id == map && n.id == *id)
            {
                Some(n) if n.source == Source::Mdt => {
                    if n.forces != *count || n.name != *name {
                        n.forces = *count;
                        n.name = name.to_string();
                        out.updated += 1;
                    }
                }
                Some(_) => {}
                None => {
                    pack.npcs.push(Npc {
                        id: *id,
                        name: name.to_string(),
                        map_id: map,
                        forces: *count,
                        source: Source::Mdt,
                        note: String::new(),
                    });
                    out.added += 1;
                }
            }
        }
        out.dungeons.push((d.name.clone(), m.total, want.len()));
    }
    crate::season::sort(pack);
    out
}

// ------------------------------------------------------------- log check

fn npc_id(u: &Unit) -> Option<u32> {
    if !(u.guid.starts_with("Creature-") || u.guid.starts_with("Vehicle-")) {
        return None;
    }
    u.guid.split('-').nth(5)?.parse().ok()
}

/// Last known health at or below this share counts as defeated.
pub const DEFEATED_HP_FRAC: f64 = 0.01;

#[derive(Default)]
struct KeyRun {
    map_id: u32,
    level: u32,
    guids: HashSet<String>,
    /// Hostile GUID -> (NPC ID, last known health fraction), for units that
    /// haven't died.
    hp: BTreeMap<String, (u32, f64)>,
    /// NPC ID -> kills.
    killed: BTreeMap<u32, u32>,
    /// NPC ID -> name, for every hostile NPC seen.
    seen: BTreeMap<u32, String>,
}

/// Collects kills per completed key from a log's events.
#[derive(Default)]
pub struct Audit {
    cur: Option<KeyRun>,
    keys: Vec<KeyRun>,
}

impl Audit {
    pub fn feed(&mut self, e: &Event) {
        match &e.kind {
            Kind::ChallengeStart { map_id, level, .. } => {
                self.cur = Some(KeyRun {
                    map_id: *map_id,
                    level: *level,
                    ..Default::default()
                });
            }
            Kind::ChallengeEnd { success, .. } => {
                if let Some(mut k) = self.cur.take()
                    && *success
                {
                    // Beaten to 1 HP and scripted away: defeated, no death logged.
                    for (g, (id, frac)) in std::mem::take(&mut k.hp) {
                        if frac <= DEFEATED_HP_FRAC && k.guids.insert(g) {
                            *k.killed.entry(id).or_default() += 1;
                        }
                    }
                    self.keys.push(k);
                }
            }
            _ => {}
        }
        let Some(k) = &mut self.cur else { return };
        for u in [&e.src, &e.dst].into_iter().flatten() {
            if u.is_hostile_npc()
                && let Some(id) = npc_id(u)
            {
                k.seen.entry(id).or_insert_with(|| u.name.clone());
            }
        }
        if let Kind::Damage { info: Some(i), .. } = &e.kind
            && let Some(d) = &e.dst
            && d.guid == i.guid
            && i.max_hp > 0
            && d.is_hostile_npc()
            && !k.guids.contains(&d.guid)
            && let Some(id) = npc_id(d)
        {
            k.hp.insert(d.guid.clone(), (id, i.hp as f64 / i.max_hp as f64));
        }
        if matches!(e.kind, Kind::Death)
            && let Some(d) = &e.dst
            && d.is_hostile_npc()
            && let Some(id) = npc_id(d)
            && k.guids.insert(d.guid.clone())
        {
            k.hp.remove(&d.guid);
            *k.killed.entry(id).or_default() += 1;
        }
    }

    pub fn report(&self, pack: &SeasonPack) -> Check {
        let mut out = Check::default();
        let mut aliases: BTreeMap<(u32, u32), Alias> = BTreeMap::new();
        let mut never: BTreeMap<(u32, u32), Missing> = BTreeMap::new();
        for k in &self.keys {
            let Some(d) = pack
                .dungeons
                .iter()
                .find(|d| d.map_id == k.map_id && d.forces_total > 0)
            else {
                continue;
            };
            let table: Vec<&Npc> = pack.npcs.iter().filter(|n| n.map_id == d.map_id).collect();
            let forces = |id: u32| table.iter().find(|n| n.id == id).map_or(0, |n| n.forces);
            let got: u32 = k.killed.iter().map(|(id, n)| forces(*id) * n).sum();
            let mut fixed = got;
            if got < d.forces_total {
                // An NPC worth forces that never died here, and a killed NPC of
                // the same name with no forces: the game logs it under another ID.
                // A same-name entry that did die (an alias already in the
                // table) covers it.
                let died_as = |n: &Npc| {
                    table.iter().any(|x| {
                        x.id != n.id
                            && x.forces > 0
                            && k.killed.contains_key(&x.id)
                            && x.name.eq_ignore_ascii_case(&n.name)
                    })
                };
                for n in table
                    .iter()
                    .filter(|n| !k.killed.contains_key(&n.id) && !died_as(n))
                {
                    let alias = k.killed.iter().find(|(id, _)| {
                        forces(**id) == 0
                            && k.seen
                                .get(id)
                                .is_some_and(|s| s.eq_ignore_ascii_case(&n.name))
                    });
                    match alias {
                        Some((id, kills)) => {
                            fixed += n.forces * kills;
                            let a = aliases.entry((d.map_id, *id)).or_insert(Alias {
                                dungeon: d.name.clone(),
                                map_id: d.map_id,
                                logged_id: *id,
                                name: n.name.clone(),
                                table_id: n.id,
                                forces: n.forces,
                                keys: 0,
                            });
                            a.keys += 1;
                        }
                        None => {
                            let m = never.entry((d.map_id, n.id)).or_insert(Missing {
                                dungeon: d.name.clone(),
                                id: n.id,
                                name: n.name.clone(),
                                forces: n.forces,
                                keys: 0,
                                seen_alive: 0,
                            });
                            m.keys += 1;
                            if k.seen.contains_key(&n.id) {
                                m.seen_alive += 1;
                            }
                        }
                    }
                }
            }
            out.keys.push(KeyCheck {
                dungeon: d.name.clone(),
                level: k.level,
                total: d.forces_total,
                forces: got,
                with_aliases: fixed,
            });
        }
        out.aliases = aliases.into_values().collect();
        out.missing = never.into_values().collect();
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub keys: Vec<KeyCheck>,
    pub aliases: Vec<Alias>,
    /// NPCs worth forces that never died in a key that fell short.
    pub missing: Vec<Missing>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyCheck {
    pub dungeon: String,
    pub level: u32,
    pub total: u32,
    pub forces: u32,
    pub with_aliases: u32,
}

/// A killed NPC with no forces that shares its name with a table NPC that
/// never died in the same short key.
#[derive(Clone, Debug, PartialEq)]
pub struct Alias {
    pub dungeon: String,
    pub map_id: u32,
    pub logged_id: u32,
    pub name: String,
    pub table_id: u32,
    pub forces: u32,
    /// Short keys it explained.
    pub keys: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Missing {
    pub dungeon: String,
    pub id: u32,
    pub name: String,
    pub forces: u32,
    /// Short keys it never died in, and how many of those it was seen in.
    pub keys: u32,
    pub seen_alive: u32,
}

fn pct(n: u32, total: u32) -> f64 {
    100.0 * n as f64 / total as f64
}

impl Check {
    pub fn render(&self) -> String {
        let mut o = String::new();
        if self.keys.is_empty() {
            o.push_str("No completed keys in dungeons with forces data.\n");
            return o;
        }
        o.push_str("Completed keys (kills worth forces / total):\n");
        let mut keys = self.keys.clone();
        keys.sort_by(|a, b| a.dungeon.cmp(&b.dungeon).then(a.level.cmp(&b.level)));
        for k in &keys {
            let fix = if k.with_aliases != k.forces {
                format!(
                    ", {:.1}% with the aliases below",
                    pct(k.with_aliases, k.total)
                )
            } else {
                String::new()
            };
            let flag = if k.with_aliases < k.total {
                "  SHORT"
            } else {
                ""
            };
            o.push_str(&format!(
                "  {:20} +{:<3} {:4}/{:<4} {:6.1}%{fix}{flag}\n",
                k.dungeon,
                k.level,
                k.forces,
                k.total,
                pct(k.forces, k.total)
            ));
        }
        if !self.aliases.is_empty() {
            o.push_str("\nProposed aliases (logged ID with no forces, same name as a table NPC that never died):\n");
            for a in &self.aliases {
                o.push_str(&format!(
                    "  {}: {} logged as {}, table has {} ({} forces); explains {} short key(s)\n",
                    a.dungeon, a.name, a.logged_id, a.table_id, a.forces, a.keys
                ));
            }
        }
        if !self.missing.is_empty() {
            o.push_str("\nNever died in short keys (no alias found):\n");
            for m in &self.missing {
                o.push_str(&format!(
                    "  {}: {} ({}, {} forces): {} key(s), seen alive in {}\n",
                    m.dungeon, m.name, m.id, m.forces, m.keys, m.seen_alive
                ));
            }
        }
        o
    }

    /// Add the proposed aliases to the pack as `source = "log"` entries.
    pub fn apply(&self, pack: &mut SeasonPack) -> usize {
        let mut n = 0;
        let have: BTreeSet<(u32, u32)> = pack.npcs.iter().map(|x| (x.map_id, x.id)).collect();
        for a in &self.aliases {
            if have.contains(&(a.map_id, a.logged_id)) {
                continue;
            }
            pack.npcs.push(Npc {
                id: a.logged_id,
                name: a.name.clone(),
                map_id: a.map_id,
                forces: a.forces,
                source: Source::Log,
                note: format!(
                    "logged ID of MDT's {} (season forces check, {} key(s))",
                    a.table_id, a.keys
                ),
            });
            n += 1;
        }
        crate::season::sort(pack);
        n
    }
}

#[cfg(test)]
mod tests;
