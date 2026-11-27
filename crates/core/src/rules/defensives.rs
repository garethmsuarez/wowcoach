//! M+ defensive advice: deaths with a personal defensive ready, damage spikes
//! that went by with a group defensive ready, enemy summons left alive, tank
//! busters taken without mitigation, and lust. Reads the records
//! `analysis/defensives.rs` produces; players are only named when every
//! check passed there.

use super::{LIST_MAX, human, mmss, sugg};
use crate::analysis::*;
use crate::classes::{AbilityKind, ClassData, Protects};
use crate::config::Config;
use crate::specs::{Role, SPECS};

/// In a pull with this many deaths or more (a wipe), only the first
/// `WIPE_COUNTED` deaths are judged: the rest had little chance.
const WIPE_DEATHS: usize = 4;
const WIPE_COUNTED: usize = 2;
/// Deaths that took less than this share of max HP in their last 5 s weren't
/// burst deaths, so a defensive wasn't the answer.
const MIN_BURST_FRAC: f64 = 0.2;
/// Tank busters: flag at least this many unmitigated hits, and this share.
const TANK_MIN_UNMITIGATED: usize = 2;
const TANK_MIN_SHARE: f64 = 0.2;

pub(super) fn apply(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    deaths_with_defensives(f, out);
    spikes(f, cfg, out);
    summons(f, cfg, out);
    tank_busters(f, out);
    lust(f, cfg, out);
}

fn pull_label(p: Option<usize>) -> String {
    p.map(|p| format!("pull {p}, ")).unwrap_or_default()
}

/// Deaths a defensive could have prevented: not wipe cascades, not the late
/// deaths of a wiping pull, and with real burst damage at the end.
pub(super) fn judged_deaths(f: &FightReport) -> Vec<&DeathReport> {
    let mut seen_in_pull: Vec<(Option<usize>, usize)> = vec![];
    let mut out = vec![];
    for d in f.deaths.iter().filter(|d| !d.wipe_cascade) {
        let in_pull = f.deaths.iter().filter(|x| x.pull == d.pull).count();
        let n = match seen_in_pull.iter_mut().find(|(p, _)| *p == d.pull) {
            Some(x) => {
                x.1 += 1;
                x.1
            }
            None => {
                seen_in_pull.push((d.pull, 1));
                1
            }
        };
        if d.pull.is_some() && in_pull >= WIPE_DEATHS && n > WIPE_COUNTED {
            continue;
        }
        if d.damage_last_5s_frac.is_some_and(|x| x < MIN_BURST_FRAC) {
            continue;
        }
        out.push(d);
    }
    out
}

fn deaths_with_defensives(f: &FightReport, out: &mut Vec<Suggestion>) {
    let judged = judged_deaths(f);
    for p in &f.players {
        let mine: Vec<&&DeathReport> = judged
            .iter()
            .filter(|d| d.player == p.name && !d.defensives_ready.is_empty())
            .collect();
        if mine.is_empty() {
            continue;
        }
        let what = |d: &DeathReport| {
            d.defensives_ready
                .iter()
                .map(|r| format!("{}, last used at {}", r.ability, mmss(r.last_used_s)))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let msg = if let [d] = mine.as_slice() {
            let names = d
                .defensives_ready
                .iter()
                .map(|r| r.ability.as_str())
                .collect::<Vec<_>>()
                .join(" and ");
            let when = d
                .defensives_ready
                .iter()
                .map(|r| mmss(r.last_used_s))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Died at {} with {names} ready (last used at {when}).",
                mmss(d.at_s)
            )
        } else {
            let list = mine
                .iter()
                .take(LIST_MAX)
                .map(|d| format!("{} ({})", mmss(d.at_s), what(d)))
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "Died {} times with a personal defensive ready: {list}{}.",
                mine.len(),
                if mine.len() > LIST_MAX { "; …" } else { "" }
            )
        };
        out.push(sugg(
            &p.name,
            if mine.len() >= 2 {
                Severity::High
            } else {
                Severity::Medium
            },
            "Defensives",
            format!("{msg} Press it when your health starts dropping, not after."),
            format!(
                "Ready = used earlier in this key, and off cooldown (the class table's, or your own faster reuse) {} s before the death. In a wiping pull only the first {WIPE_COUNTED} deaths count.",
                DEATH_REACTION_MS / 1000
            ),
        ));
    }
}

fn led_by_summon(s: &Spike) -> bool {
    s.top.first().is_some_and(|t| t.summoned_by.is_some())
}

fn spikes(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let mut missed: Vec<&Spike> = f
        .spikes
        .iter()
        .filter(|s| s.group_used.is_empty() && !s.group_ready.is_empty() && !led_by_summon(s))
        .collect();
    if missed.is_empty() {
        return;
    }
    missed.sort_by(|a, b| {
        b.peak_frac
            .total_cmp(&a.peak_frac)
            .then(a.start_s.total_cmp(&b.start_s))
    });
    let list = missed
        .iter()
        .take(LIST_MAX)
        .map(|s| {
            let ready = s
                .group_ready
                .iter()
                .map(|r| format!("{} ({})", r.ability, r.player))
                .collect::<Vec<_>>()
                .join(", ");
            let died = if s.deaths.is_empty() {
                String::new()
            } else {
                format!(", {} died", s.deaths.len())
            };
            format!(
                "{} ({}{:.0}% of group HP, mostly {}{died}): {ready}",
                mmss(s.start_s),
                pull_label(s.pull),
                s.peak_frac * 100.0,
                s.top.first().map(|t| t.name.as_str()).unwrap_or("?")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let g = &cfg.general;
    out.push(sugg(
        "group",
        if missed.iter().any(|s| !s.deaths.is_empty()) {
            Severity::High
        } else {
            Severity::Medium
        },
        "Spikes",
        format!(
            "{} damage spike(s) went by without a group defensive while one was ready, worst first: {list}{}. Agree before the key who covers which dangerous pull.",
            missed.len(),
            if missed.len() > LIST_MAX { "; …" } else { "" }
        ),
        format!(
            "A spike = the group losing {:.0}% of its summed max HP within {} s (after absorbs). Ready = used earlier in this key and off cooldown. Spikes driven by enemy summons are under Summons.",
            g.spike_frac * 100.0,
            g.spike_window_ms / 1000
        ),
    ));
}

fn summons(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let max_s = cfg.general.summon_alive_max_s;
    // Group by (summon, summoner); only summons that hurt the group or healed enemies.
    let mut groups: Vec<((&str, &str), Vec<&EnemySummon>)> = vec![];
    for s in f
        .enemy_summons
        .iter()
        .filter(|s| s.damage_to_group > 0 || s.healing_to_enemies > 0)
    {
        let k = (s.name.as_str(), s.summoner.as_str());
        match groups.iter_mut().find(|g| g.0 == k) {
            Some(g) => g.1.push(s),
            None => groups.push((k, vec![s])),
        }
    }
    let mut flagged: Vec<(u64, String, bool)> = vec![];
    for ((name, summoner), v) in &groups {
        let mut killed: Vec<f64> = v
            .iter()
            .filter(|s| s.killed)
            .map(|s| s.end_s - s.start_s)
            .collect();
        killed.sort_by(f64::total_cmp);
        let med = killed.get(killed.len() / 2).copied();
        let slow = killed.iter().filter(|l| **l > max_s).count();
        let unkilled = v.len() - killed.len();
        // A late kill now and then is normal; flag when the typical one lives
        // too long, or none were killed at all.
        if med.is_some_and(|m| m <= max_s) {
            continue;
        }
        let damage: u64 = v.iter().map(|s| s.damage_to_group).sum();
        let healing: u64 = v.iter().map(|s| s.healing_to_enemies).sum();
        // Spikes this summon led (their top damage source).
        let in_spikes = f
            .spikes
            .iter()
            .filter(|s| {
                s.top.first().is_some_and(|t| {
                    t.source == *name && t.summoned_by.as_deref() == Some(*summoner)
                })
            })
            .count();
        // Adds the group cleaves down anyway (hatchlings, boss adds that
        // despawn) live long too; they only matter here when they lead a
        // spike or heal enemies.
        if in_spikes == 0 && healing == 0 {
            continue;
        }
        let alive = match med {
            Some(m) => format!(
                "killed after a median {m:.0} s ({slow} lived over {max_s:.0} s{})",
                if unkilled > 0 {
                    format!(", {unkilled} not killed")
                } else {
                    String::new()
                }
            ),
            None => "none killed".into(),
        };
        let mut effect = vec![];
        if damage > 0 {
            effect.push(format!("{} damage to the group", human(damage)));
        }
        if healing > 0 {
            effect.push(format!("{} healing to enemies", human(healing)));
        }
        let spikes = if in_spikes > 0 {
            format!(", leading {in_spikes} damage spike(s)")
        } else {
            String::new()
        };
        flagged.push((
            damage + healing,
            format!(
                "{name} ({summoner}): {} summoned, {alive}; {}{spikes}",
                v.len(),
                effect.join(" and ")
            ),
            in_spikes > 0,
        ));
    }
    if flagged.is_empty() {
        return;
    }
    flagged.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let list = flagged
        .iter()
        .take(LIST_MAX)
        .map(|x| x.1.as_str())
        .collect::<Vec<_>>()
        .join(". ");
    out.push(sugg(
        "group",
        if flagged.iter().any(|x| x.2) {
            Severity::High
        } else {
            Severity::Medium
        },
        "Summons",
        format!("Enemy summons stayed up too long. {list}. Kill them as soon as they drop."),
        format!(
            "Units enemies summoned (totems, adds) that led a damage spike or healed enemies. Flagged when the typical one lived over {max_s:.0} s, or none were killed (a summon that expires isn't logged as killed)."
        ),
    ));
}

fn tank_busters(f: &FightReport, out: &mut Vec<Suggestion>) {
    let classes = ClassData::shared();
    for p in f.players.iter().filter(|p| p.role == Role::Tank) {
        let hits: Vec<&TankHit> = f.tank_hits.iter().filter(|h| h.tank == p.name).collect();
        let bare: Vec<&&TankHit> = hits
            .iter()
            .filter(|h| h.mitigation.is_empty() && h.defensives.is_empty())
            .collect();
        if bare.len() < TANK_MIN_UNMITIGATED
            || (bare.len() as f64) < TANK_MIN_SHARE * hits.len() as f64
        {
            continue;
        }
        let mut by_spell: Vec<(&str, usize)> = vec![];
        for h in &bare {
            match by_spell.iter_mut().find(|x| x.0 == h.name) {
                Some(x) => x.1 += 1,
                None => by_spell.push((&h.name, 1)),
            }
        }
        by_spell.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let spells = by_spell
            .iter()
            .map(|(n, k)| format!("{n} ×{k}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mitigation: Vec<String> = SPECS
            .iter()
            .find(|s| s.1 == p.class && s.2 == p.spec)
            .map(|s| classes.for_spec(s.0, AbilityKind::Mitigation))
            .unwrap_or_default()
            .into_iter()
            .map(|a| match a.school {
                Some(Protects::Physical) => format!("{} (physical only)", a.name),
                Some(Protects::Magic) => format!("{} (magic only)", a.name),
                None => a.name.clone(),
            })
            .collect();
        let have = if mitigation.is_empty() {
            "active mitigation".to_string()
        } else {
            mitigation.join(" or ")
        };
        out.push(sugg(
            &p.name,
            if bare.len() * 2 >= hits.len() {
                Severity::High
            } else {
                Severity::Medium
            },
            "Tank busters",
            format!(
                "{} of {} tank busters hit with no active mitigation or defensive up: {spells}. Have {have} up before they land.",
                bare.len(),
                hits.len()
            ),
            "Tank busters are the season pack's `tank_buster` spells. Mitigation only counts against the damage it works on.".into(),
        ));
    }
}

fn lust(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let FightKind::MythicPlus { bosses, .. } = &f.kind else {
        return;
    };
    let missed: Vec<&BossMarker> = bosses
        .iter()
        .filter(|b| b.lust_ready && !b.lust_used)
        .collect();
    if !missed.is_empty() {
        let list = missed
            .iter()
            .map(|b| format!("{} at {}", b.name, mmss(b.start_s)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(sugg(
            "group",
            Severity::Medium,
            "Lust",
            format!("Lust was available and not used on: {list}."),
            "Available = a lust class in the group and nobody Sated when the boss was engaged."
                .into(),
        ));
    }

    // Lust sitting ready through dangerous trash: in-combat time on pulls with
    // a damage spike, counted from when it became available until it's used.
    let max_s = cfg.general.lust_idle_max_s;
    let mut idle: Vec<(usize, usize, f64)> = vec![];
    let (mut acc, mut from, mut flagged) = (0.0, None, false);
    for p in &f.pulls {
        if p.lust_used {
            (acc, from, flagged) = (0.0, None, false);
            continue;
        }
        if p.spikes > 0 && p.lust_ready_s > 0.0 && p.boss.is_none() {
            acc += p.lust_ready_s;
            let first = *from.get_or_insert(p.index);
            if acc > max_s && !flagged {
                idle.push((first, p.index, acc));
                flagged = true;
            } else if flagged && let Some(x) = idle.last_mut() {
                (x.1, x.2) = (p.index, acc);
            }
        }
    }
    if !idle.is_empty() {
        let list = idle
            .iter()
            .map(|(a, b, s)| {
                let pulls = if a == b {
                    format!("pull {a}")
                } else {
                    format!("pulls {a}–{b}")
                };
                format!("{} of combat over {pulls}", mmss(*s))
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(sugg(
            "group",
            Severity::Medium,
            "Lust",
            format!(
                "Lust sat unused through dangerous trash: {list}. Using it on a pack that's spiking the group saves time and deaths."
            ),
            format!(
                "Counted: in-combat time with lust available, on trash pulls with a damage spike, from when lust came back until it was used. Flagged past {}.",
                mmss(max_s)
            ),
        ));
    }

    if f.pulls.first().is_some_and(|p| p.lust_used) {
        out.push(sugg(
            "group",
            Severity::Low,
            "Lust",
            "Lust went out on the first pull. That's a valid choice for a fast start; it means any boss in the next 10 minutes goes without it.".into(),
            String::new(),
        ));
    }
}
