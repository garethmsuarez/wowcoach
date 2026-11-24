//! Streaming parser for retail `WoWCombatLog*.txt` files.
//!
//! Line shape: `M/D/YYYY HH:MM:SS.ffff-TZ  EVENT,field,field,...`
//! (older logs omit the year and timezone; both are accepted).

use crate::model::*;
use std::io::BufRead;

/// Default number of "advanced combat logging" fields (infoGUID .. level).
/// Midnight (12.x) logs use 19; The War Within used 17. The parser re-learns
/// the real count from the first SPELL_CAST_SUCCESS line (it has no suffix).
pub const DEFAULT_ADV_FIELDS: usize = 19;

/// Split a combat-log payload on top-level commas, respecting quotes and
/// nested `[]` / `()` (used by COMBATANT_INFO and CHALLENGE_MODE_START).
pub fn split_fields(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(40);
    let (mut depth, mut in_quote, mut start) = (0i32, false, 0usize);
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b'[' | b'(' if !in_quote => depth += 1,
            b']' | b')' if !in_quote => depth -= 1,
            b',' if !in_quote && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim_end());
    out
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
}
fn num_u(s: &str) -> u64 {
    s.parse::<i64>().map(|v| v.max(0) as u64).unwrap_or(0)
}
fn num_u32(s: &str) -> u32 {
    s.parse::<u32>().unwrap_or(0)
}
fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse "11/23/2026 21:14:05.1234-6" (or "4/22 20:41:42.123") into ms.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let (date, time) = s.split_once(' ')?;
    let mut dp = date.split('/');
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    let y: i64 = dp.next().and_then(|y| y.parse().ok()).unwrap_or(2000);
    // strip timezone suffix (+N / -N) after the fractional seconds
    let time = match time.rfind(['-', '+']) {
        Some(i) if i > 0 => &time[..i],
        _ => time,
    };
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let sec = tp.next()?;
    let (ss, frac) = sec.split_once('.').unwrap_or((sec, "0"));
    let ss: i64 = ss.parse().ok()?;
    let mut ms_str: String = frac.chars().take(3).collect();
    while ms_str.len() < 3 {
        ms_str.push('0');
    }
    let ms: i64 = ms_str.parse().ok()?;
    Some((days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss) * 1000 + ms)
}

fn unit(guid: &str, name: &str, flags: &str) -> Option<Unit> {
    if guid == "0000000000000000" || guid.is_empty() || guid == "nil" {
        return None;
    }
    Some(Unit {
        guid: guid.to_string(),
        name: unquote(name).to_string(),
        flags: hex(flags),
    })
}

fn spell_at(f: &[&str], i: usize) -> Option<Spell> {
    Some(Spell {
        id: f.get(i)?.parse().ok()?,
        name: unquote(f.get(i + 1)?).to_string(),
    })
}

fn adv_at(f: &[&str], i: usize, n: usize) -> Option<AdvInfo> {
    if f.len() < i + n {
        return None;
    }
    Some(AdvInfo {
        guid: f[i].to_string(),
        owner_guid: f[i + 1].to_string(),
        hp: num_u(f[i + 2]),
        max_hp: num_u(f[i + 3]),
    })
}

/// Stateful line parser (tracks whether advanced logging is on).
pub struct Parser {
    pub advanced: bool,
    pub adv_fields: usize,
    adv_learned: bool,
    pub log_version: Option<u32>,
    pub lines: u64,
    pub skipped: u64,
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            advanced: true,
            adv_fields: DEFAULT_ADV_FIELDS,
            adv_learned: false,
            log_version: None,
            lines: 0,
            skipped: 0,
        }
    }
}

impl Parser {
    pub fn parse_line(&mut self, line: &str) -> Option<Event> {
        self.lines += 1;
        let (ts_str, payload) = line.split_once("  ")?;
        let ts = parse_timestamp(ts_str)?;
        let f = split_fields(payload);
        let ev = *f.first()?;

        match ev {
            "COMBAT_LOG_VERSION" => {
                self.log_version = f.get(1).and_then(|v| v.parse().ok());
                if let Some(p) = f.iter().position(|x| *x == "ADVANCED_LOG_ENABLED") {
                    self.advanced = f.get(p + 1) == Some(&"1");
                }
                return None;
            }
            "ENCOUNTER_START" => {
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::EncounterStart {
                        id: num_u32(f.get(1)?),
                        name: unquote(f.get(2)?).into(),
                        difficulty: num_u32(f.get(3)?),
                        group_size: num_u32(f.get(4)?),
                    },
                });
            }
            "ENCOUNTER_END" => {
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::EncounterEnd {
                        id: num_u32(f.get(1)?),
                        name: unquote(f.get(2)?).into(),
                        difficulty: num_u32(f.get(3)?),
                        success: f.get(5) == Some(&"1"),
                        duration_ms: f.get(6).and_then(|v| v.parse().ok()).unwrap_or(0),
                    },
                });
            }
            "CHALLENGE_MODE_START" => {
                let affixes = f
                    .get(5)
                    .map(|s| {
                        s.trim_matches(['[', ']'])
                            .split(',')
                            .filter_map(|a| a.trim().parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::ChallengeStart {
                        zone: unquote(f.get(1)?).into(),
                        map_id: num_u32(f.get(2)?),
                        level: num_u32(f.get(4)?),
                        affixes,
                    },
                });
            }
            "CHALLENGE_MODE_END" => {
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::ChallengeEnd {
                        map_id: num_u32(f.get(1)?),
                        success: f.get(2) == Some(&"1"),
                        level: num_u32(f.get(3)?),
                        time_ms: f.get(4).and_then(|v| v.parse().ok()).unwrap_or(0),
                    },
                });
            }
            "ZONE_CHANGE" => {
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::ZoneChange {
                        instance_id: num_u32(f.get(1)?),
                    },
                });
            }
            "COMBATANT_INFO" => {
                // GUID, faction, <stats...>, armor, currentSpecID, [talents], ...
                // The stat count changes between expansions, so take the field
                // just before the first bracketed (talent) list.
                let first_list = f.iter().position(|x| x.starts_with('['))?;
                return Some(Event {
                    ts,
                    src: None,
                    dst: None,
                    kind: Kind::Combatant {
                        guid: f.get(1)?.to_string(),
                        spec_id: num_u32(f.get(first_list.checked_sub(1)?)?),
                    },
                });
            }
            _ => {}
        }

        // Everything below is a standard combat event with 8 unit fields.
        if f.len() < 9 || ev.ends_with("_SUPPORT") {
            self.skipped += 1;
            return None;
        }
        let src = unit(f[1], f[2], f[3]);
        let dst = unit(f[5], f[6], f[7]);

        // SPELL_ABSORBED: attacker, protected unit, [attacker spell (3 fields)
        // if not melee], absorber GUID/name/flags/raidflags, shield id/name/school,
        // amount, unmitigated total, critical. Index from the end so both
        // variants parse the same way.
        if ev == "SPELL_ABSORBED" {
            let n = f.len();
            if n < 19 {
                self.skipped += 1;
                return None;
            }
            let absorber = unit(f[n - 10], f[n - 9], f[n - 8])?;
            return Some(Event {
                ts,
                src,
                dst,
                kind: Kind::Absorbed {
                    absorber,
                    spell: spell_at(&f, n - 6)?,
                    amount: num_u(f[n - 3]),
                },
            });
        }

        let (prefix_end, spell) = if ev.starts_with("SPELL_") || ev.starts_with("RANGE_") {
            (12, spell_at(&f, 9))
        } else if ev.starts_with("SWING_") {
            (9, Some(Spell::melee()))
        } else if ev.starts_with("ENVIRONMENTAL_") {
            (9, None)
        } else if ev == "UNIT_DIED" {
            // Trailing `unconsciousOnDeath` = 1 marks Feign Death (hunters dropping
            // debuff stacks, etc.): the player didn't actually die, so skip it.
            if f.get(9).map(|s| s.trim()) == Some("1") {
                self.skipped += 1;
                return None;
            }
            return Some(Event {
                ts,
                src,
                dst,
                kind: Kind::Death,
            });
        } else {
            self.skipped += 1;
            return None;
        };

        // SPELL_CAST_SUCCESS = 12 base fields + the advanced block, nothing after.
        if self.advanced && !self.adv_learned && ev == "SPELL_CAST_SUCCESS" {
            let n = f.len().saturating_sub(12);
            if (15..=25).contains(&n) {
                self.adv_fields = n;
                self.adv_learned = true;
            }
        }
        let (adv, n_adv) = (self.advanced, self.adv_fields);
        let with_adv = |i: usize| -> (Option<AdvInfo>, usize) {
            if adv {
                (adv_at(&f, i, n_adv), i + n_adv)
            } else {
                (None, i)
            }
        };

        let kind = match ev {
            "SPELL_DAMAGE"
            | "SPELL_PERIODIC_DAMAGE"
            | "RANGE_DAMAGE"
            | "SWING_DAMAGE"
            | "SPELL_BUILDING_DAMAGE"
            | "ENVIRONMENTAL_DAMAGE" => {
                let (info, mut s) = with_adv(prefix_end);
                let spell = match spell {
                    Some(sp) => sp,
                    None => {
                        // environmental: type string precedes the damage suffix
                        let t = unquote(f.get(s)?).to_string();
                        s += 1;
                        Spell {
                            id: 0,
                            name: format!("Environment ({t})"),
                        }
                    }
                };
                Kind::Damage {
                    spell,
                    amount: num_u(f.get(s)?),
                    overkill: f.get(s + 2).map(|v| num_u(v)).unwrap_or(0),
                    absorbed: f.get(s + 6).map(|v| num_u(v)).unwrap_or(0),
                    periodic: ev == "SPELL_PERIODIC_DAMAGE",
                    info,
                }
            }
            "SPELL_HEAL" | "SPELL_PERIODIC_HEAL" => {
                let (info, s) = with_adv(prefix_end);
                Kind::Heal {
                    spell: spell?,
                    amount: num_u(f.get(s)?),
                    overheal: f.get(s + 2).map(|v| num_u(v)).unwrap_or(0),
                    absorbed: f.get(s + 3).map(|v| num_u(v)).unwrap_or(0),
                    info,
                }
            }
            "SPELL_CAST_START" => Kind::CastStart { spell: spell? },
            "SPELL_CAST_SUCCESS" => {
                let (info, _) = with_adv(prefix_end);
                Kind::CastSuccess {
                    spell: spell?,
                    info,
                }
            }
            "SPELL_CAST_FAILED" => Kind::CastFailed { spell: spell? },
            "SPELL_INTERRUPT" => Kind::Interrupt {
                spell: spell?,
                extra: spell_at(&f, 12)?,
            },
            "SPELL_DISPEL" | "SPELL_STOLEN" => Kind::Dispel {
                spell: spell?,
                extra: spell_at(&f, 12)?,
            },
            "SPELL_AURA_APPLIED" | "SPELL_AURA_REMOVED" => {
                let is_buff = f.get(12) == Some(&"BUFF");
                if ev == "SPELL_AURA_APPLIED" {
                    Kind::AuraApplied {
                        spell: spell?,
                        is_buff,
                    }
                } else {
                    Kind::AuraRemoved {
                        spell: spell?,
                        is_buff,
                    }
                }
            }
            "SPELL_SUMMON" => Kind::Summon { spell: spell? },
            _ => {
                self.skipped += 1;
                return None;
            }
        };
        Some(Event { ts, src, dst, kind })
    }
}

/// Stream every event in a reader into `sink`. Invalid UTF-8 is replaced.
pub fn stream<R: BufRead>(mut r: R, mut sink: impl FnMut(Event)) -> std::io::Result<Parser> {
    let mut p = Parser::default();
    let mut buf = Vec::with_capacity(1024);
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(e) = p.parse_line(line) {
            sink(e);
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nested() {
        let f = split_fields(r#"A,"x, y",[1,(2,3)],4"#);
        assert_eq!(f, vec!["A", "\"x, y\"", "[1,(2,3)]", "4"]);
    }

    #[test]
    fn timestamps() {
        let a = parse_timestamp("11/23/2026 21:14:05.1234-6").unwrap();
        let b = parse_timestamp("11/23/2026 21:14:06.000-6").unwrap();
        assert_eq!(b - a, 877);
        assert!(parse_timestamp("4/22 20:41:42.123").is_some());
    }

    #[test]
    fn parses_spell_damage() {
        let mut p = Parser::default();
        let line = r#"11/23/2026 21:14:05.123-6  SPELL_DAMAGE,Creature-0-1-2-3-4-5,"Boss",0xa48,0x0,Player-1-ABC,"Tanky-Realm",0x512,0x0,12345,"Fire Puddle",0x4,Player-1-ABC,0000000000000000,80000,100000,0,0,0,0,0,0,0,0,0,0,0,0,0,2000,70,20000,19000,-1,4,0,0,500,nil,nil,nil,AOE"#;
        let e = p.parse_line(line).unwrap();
        match e.kind {
            Kind::Damage {
                spell,
                amount,
                overkill,
                absorbed,
                info,
                ..
            } => {
                assert_eq!(spell.id, 12345);
                assert_eq!(amount, 20000);
                assert_eq!(overkill, 0);
                assert_eq!(absorbed, 500);
                assert_eq!(info.unwrap().hp, 80000);
            }
            k => panic!("{k:?}"),
        }
        assert!(e.dst.unwrap().is_group_player());
    }

    /// Real 12.1.5 lines (names anonymised): 19 advanced fields, 21 stats.
    #[test]
    fn midnight_12_1_layout() {
        let mut p = Parser::default();
        let lines = [
            r#"11/22/2026 15:37:00.000-6  SPELL_CAST_SUCCESS,Player-1-00000001,"Tank-Realm-US",0x511,0x80000000,0000000000000000,nil,0x80000000,0x80000000,131347,"Glide",0x1,Player-1-00000001,0000000000000000,1480099,1480099,4212,622,5488,859,0,0,17,0,120,0,1278.15,2153.02,2500,4.9972,332"#,
            r#"11/22/2026 15:37:01.000-6  SPELL_DAMAGE,Player-1-00000001,"Tank-Realm-US",0x511,0x80000000,Creature-0-1-2859-1-245410-0000000001,"Lasher",0xa48,0x80000000,204598,"Sigil of Flame",0x4,Creature-0-1-2859-1-245410-0000000001,0000000000000000,1899424,1907034,0,0,1470,0,0,0,1,0,0,0,1317.03,2070.76,2500,4.9988,90,7610,7610,-1,4,0,0,0,nil,nil,nil,AOE"#,
            r#"11/22/2026 15:37:02.000-6  SPELL_HEAL,Player-1-00000002,"Healer-Realm-US",0x512,0x80000000,Player-1-00000001,"Tank-Realm-US",0x511,0x80000000,1246798,"Prompt Prognosis",0x2,Player-1-00000001,0000000000000000,1554108,1554108,4524,622,5549,859,0,65948,17,0,120,0,1292.46,2117.62,2500,5.2584,332,123825,123825,100000,0,nil"#,
            r#"11/22/2026 15:37:03.000-6  COMBATANT_INFO,Player-1-00000001,0,512,2674,73307,622,0,0,618,0,618,618,618,65,28,1149,1149,1149,0,883,464,464,464,5549,581,[(90929,112839,1)],(0,0,0,0),[],[],[]"#,
        ];
        let evs: Vec<Event> = lines.iter().filter_map(|l| p.parse_line(l)).collect();
        assert_eq!(p.adv_fields, 19);
        match &evs[1].kind {
            Kind::Damage { amount, info, .. } => {
                assert_eq!(*amount, 7610);
                assert_eq!(info.as_ref().unwrap().max_hp, 1907034);
            }
            k => panic!("{k:?}"),
        }
        assert!(matches!(
            evs[2].kind,
            Kind::Heal {
                amount: 123825,
                overheal: 100000,
                ..
            }
        ));
        assert!(matches!(&evs[3].kind, Kind::Combatant { spec_id: 581, .. }));
    }

    #[test]
    fn feign_death_is_not_a_death() {
        let mut p = Parser::default();
        let fd = r#"11/23/2026 20:53:30.873-6  UNIT_DIED,0000000000000000,nil,0x80000000,0x80000000,Player-1-00000003,"Hunter-Realm-US",0x512,0x80000000,1"#;
        let real = r#"11/23/2026 20:53:31.000-6  UNIT_DIED,0000000000000000,nil,0x80000000,0x80000000,Player-1-00000003,"Hunter-Realm-US",0x512,0x80000000,0"#;
        assert!(p.parse_line(fd).is_none());
        assert!(matches!(p.parse_line(real).unwrap().kind, Kind::Death));
    }

    /// Real 12.1.5 SPELL_ABSORBED lines (names anonymised): melee and spell variants.
    #[test]
    fn absorbs_credit_the_shield_caster() {
        let mut p = Parser::default();
        let melee = r#"11/22/2026 15:37:00.000-6  SPELL_ABSORBED,Creature-0-1-2859-1-245336-0000000002,"Radiant Spellsower",0x10a48,0x80000000,Player-1-00000001,"Tank-Realm-US",0x511,0x80000000,Player-1-00000002,"Priest-Realm-US",0x512,0x80000000,1246768,"Power Word: Shield",0x2,65948,408279,nil"#;
        let spell = r#"11/22/2026 15:37:01.000-6  SPELL_ABSORBED,Creature-0-1-2-3-4-5,"Boss",0xa48,0x80000000,Player-1-00000001,"Tank-Realm-US",0x511,0x80000000,1238063,"Light Bolt",0x2,Player-1-00000002,"Priest-Realm-US",0x512,0x80000000,1246768,"Power Word: Shield",0x2,7787,9000,nil"#;
        for (line, amt) in [(melee, 65948), (spell, 7787)] {
            match p.parse_line(line).unwrap().kind {
                Kind::Absorbed {
                    absorber,
                    spell,
                    amount,
                } => {
                    assert_eq!(absorber.name, "Priest-Realm-US");
                    assert_eq!(spell.name, "Power Word: Shield");
                    assert_eq!(amount, amt);
                }
                k => panic!("{k:?}"),
            }
        }
    }
}
