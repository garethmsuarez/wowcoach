//! M+ route and pacing advice (M5): the timer margin, forces at each boss
//! against top runs, overshoot at the end, and the segments that lost the
//! most time. Reads `FightReport.segments`, pull forces and, with
//! `analyze --compare-top`, `FightReport.reference`.

use super::{LIST_MAX, mmss, sugg};
use crate::analysis::*;
use crate::config::Config;
use crate::reference::{RefSegment, TopRuns};

pub(super) fn apply(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    timer_margin(f, cfg, out);
    overshoot(f, cfg, out);
    match &f.reference {
        Some(r) => {
            route(f, r, cfg, out);
            segments(f, r, cfg, out);
        }
        None => slow_pulls(f, out),
    }
}

/// "start → Kystia", "Kystia → Zaen", "after Lithiel".
fn segment_label(f: &FightReport, s: &Segment) -> String {
    let prev = f
        .segments
        .iter()
        .find(|p| p.index + 1 == s.index)
        .and_then(|p| p.boss.as_deref());
    match (prev, s.boss.as_deref()) {
        (None, Some(b)) => format!("start → {b}"),
        (Some(a), Some(b)) => format!("{a} → {b}"),
        (Some(a), None) => format!("after {a}"),
        (None, None) => "the key".into(),
    }
}

/// Trash pace: % forces per minute of trash combat (boss fights excluded).
fn trash_rate_per_s(f: &FightReport) -> Option<f64> {
    let pct = f.forces_pct?;
    let trash_s: f64 = f.segments.iter().map(|s| s.combat_s - s.boss_s).sum();
    (trash_s > 0.0).then(|| pct / trash_s)
}

/// Where the time went, biggest first: death penalties, downtime, and with
/// top runs the segment that lost the most.
pub(super) fn levers(f: &FightReport) -> String {
    let mut v: Vec<(f64, String)> = vec![];
    let penalty: f64 = f.segments.iter().map(|s| s.death_penalty_s).sum();
    let deaths: u32 = f.segments.iter().map(|s| s.deaths).sum();
    if penalty > 0.0 {
        v.push((
            penalty,
            format!("death penalty {} ({deaths} deaths)", mmss(penalty)),
        ));
    }
    if f.downtime_s > 0.0 {
        v.push((f.downtime_s, format!("downtime {}", mmss(f.downtime_s))));
    }
    if let Some(r) = &f.reference
        && let Some((s, d)) = f
            .segments
            .iter()
            .filter_map(|s| Some((s, s.end_s - s.start_s - ref_seg(r, s)?.duration_s.median)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        && d > 0.0
    {
        v.push((
            d,
            format!(
                "{} {} slower than the top runs",
                segment_label(f, s),
                mmss(d)
            ),
        ));
    }
    if v.is_empty() {
        return String::new();
    }
    v.sort_by(|a, b| b.0.total_cmp(&a.0));
    format!(
        "Biggest levers in this key: {}.",
        v.into_iter().map(|x| x.1).collect::<Vec<_>>().join(", ")
    )
}

/// Timed keys: the margin, and how far from the next upgrade.
fn timer_margin(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let FightKind::MythicPlus {
        map_id,
        time_s,
        upgrades: Some(u),
        ..
    } = &f.kind
    else {
        return;
    };
    if *u == 0 {
        return;
    }
    let Some(timers) = cfg
        .knowledge
        .pack
        .as_ref()
        .and_then(|p| p.dungeon_by_map(*map_id))
        .map(|d| d.timer_s.clone())
    else {
        return;
    };
    let u = *u as usize;
    let spare = timers.get(u - 1).map(|t| *t as f64 - time_s).unwrap_or(0.0);
    let mut msg = format!("+{u} with {} to spare", mmss(spare));
    if let Some(next) = timers.get(u) {
        msg.push_str(&format!(
            "; {} short of +{}",
            mmss(time_s - *next as f64),
            u + 1
        ));
    }
    msg.push('.');
    out.push(sugg("group", Severity::Low, "Timer", msg, levers(f)));
}

/// More than 100% forces: the extra trash, costed at this key's trash pace.
fn overshoot(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let (Some(pct), Some(rate)) = (f.forces_pct, trash_rate_per_s(f)) else {
        return;
    };
    let extra = pct - 100.0;
    if extra <= cfg.general.forces_over_pct {
        return;
    }
    let cost = extra / rate;
    out.push(sugg(
        "group",
        if cost >= 60.0 {
            Severity::Medium
        } else {
            Severity::Low
        },
        "Overshoot",
        format!(
            "Killed {pct:.0}% enemy forces, {extra:.0}% more than needed: about {} of trash at this key's pace ({:.1}% a minute). Skip a pack, or pull a smaller one instead.",
            mmss(cost),
            rate * 60.0
        ),
        "Enemy forces from the season pack's table (Mythic Dungeon Tools). Pace = forces per minute of trash combat.".into(),
    ));
}

fn ref_seg<'a>(r: &'a TopRuns, s: &Segment) -> Option<&'a RefSegment> {
    r.segments
        .iter()
        .find(|x| x.index == s.index && x.boss == s.boss)
}

fn top_label(r: &TopRuns) -> String {
    format!("{} top +{} run(s) on Warcraft Logs", r.runs.len(), r.level)
}

/// Forces well behind the top runs at a boss: that trash came later (or
/// was extra).
fn route(f: &FightReport, r: &TopRuns, cfg: &Config, out: &mut Vec<Suggestion>) {
    let FightKind::MythicPlus { bosses, .. } = &f.kind else {
        return;
    };
    let behind: Vec<String> = r
        .bosses
        .iter()
        .filter_map(|rb| {
            let mine = bosses
                .iter()
                .find(|b| b.success && b.encounter_id == rb.encounter_id)?
                .forces_pct?;
            let top = rb.forces_pct?;
            // At 100% there's nothing left to get, however much the top runs
            // over-pulled.
            (mine < 100.0 && top.median.min(100.0) - mine > cfg.general.forces_gap_pct).then(|| {
                format!(
                    "{} {mine:.0}% (top runs {:.0}%, {:.0}–{:.0}%)",
                    rb.name, top.median, top.min, top.max
                )
            })
        })
        .collect();
    if behind.is_empty() {
        return;
    }
    out.push(sugg(
        "group",
        Severity::Medium,
        "Route",
        format!(
            "Behind the top runs on enemy forces at: {}. That trash was pulled later instead: look at pulling it on the way, so the last stretch is shorter.",
            behind.join("; ")
        ),
        format!(
            "Forces killed by each boss kill, against {}. Forces from the season pack's table (Mythic Dungeon Tools).",
            top_label(r)
        ),
    ));
}

/// Segments that lost the most time against the top runs, with the split
/// and the slowest pulls.
fn segments(f: &FightReport, r: &TopRuns, cfg: &Config, out: &mut Vec<Suggestion>) {
    let mut slow: Vec<(f64, String)> = f
        .segments
        .iter()
        .filter_map(|s| {
            let t = ref_seg(r, s)?;
            let mine = s.end_s - s.start_s;
            let delta = mine - t.duration_s.median;
            if delta <= cfg.general.segment_slow_s || mine <= 1.2 * t.duration_s.median {
                return None;
            }
            let deaths = if s.deaths > 0 {
                format!(", {} death(s) ({} penalty)", s.deaths, mmss(s.death_penalty_s))
            } else {
                String::new()
            };
            let mut pulls: Vec<&PullReport> = f
                .pulls
                .iter()
                .filter(|p| s.pulls.contains(&p.index))
                .collect();
            pulls.sort_by(|a, b| (b.end_s - b.start_s).total_cmp(&(a.end_s - a.start_s)));
            let slowest = pulls
                .iter()
                .take(2)
                .map(|p| {
                    let len = p.end_s - p.start_s;
                    let what = match (&p.boss, p.forces_pct) {
                        (Some(b), _) => b.clone(),
                        (None, Some(fp)) if len > 0.0 => {
                            format!("{fp:.0}% forces, {:.1}%/min", fp * 60.0 / len)
                        }
                        _ => "trash".into(),
                    };
                    format!("pull {} {} ({what})", p.index, mmss(len))
                })
                .collect::<Vec<_>>()
                .join(", ");
            Some((
                delta,
                format!(
                    "{}: {} against {} (+{}). Combat {} vs {}, downtime {} vs {}{deaths}. Longest pulls: {slowest}",
                    segment_label(f, s),
                    mmss(mine),
                    mmss(t.duration_s.median),
                    mmss(delta),
                    mmss(s.combat_s),
                    mmss(t.combat_s.median),
                    mmss(s.downtime_s),
                    mmss(t.downtime_s.median),
                ),
            ))
        })
        .collect();
    if slow.is_empty() {
        return;
    }
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    let worst = slow[0].0;
    out.push(sugg(
        "group",
        if worst > 120.0 {
            Severity::High
        } else {
            Severity::Medium
        },
        "Segments",
        format!(
            "Slowest stretches against the top runs, worst first. {}{}.",
            slow.iter()
                .take(LIST_MAX)
                .map(|x| x.1.clone())
                .collect::<Vec<_>>()
                .join(". "),
            if slow.len() > LIST_MAX { ". …" } else { "" }
        ),
        format!(
            "Segments run from one boss kill to the next; top = the median of {}. Combat time includes the boss fight. More combat time means slower kills or bigger pulls; more downtime means gaps between pulls.",
            top_label(r)
        ),
    ));
}

/// Without top runs: trash pulls killing forces at under half the key's
/// usual pace.
fn slow_pulls(f: &FightReport, out: &mut Vec<Suggestion>) {
    let rates: Vec<(&PullReport, f64)> = f
        .pulls
        .iter()
        .filter(|p| p.boss.is_none() && p.end_s - p.start_s >= 30.0)
        .filter_map(|p| Some((p, p.forces_pct? * 60.0 / (p.end_s - p.start_s))))
        .filter(|(_, r)| *r > 0.0)
        .collect();
    if rates.len() < 3 {
        return;
    }
    let mut sorted: Vec<f64> = rates.iter().map(|x| x.1).collect();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let mut slow: Vec<&(&PullReport, f64)> = rates
        .iter()
        .filter(|(p, r)| *r < 0.5 * median && p.end_s - p.start_s >= 60.0)
        .collect();
    if slow.is_empty() {
        return;
    }
    slow.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.index.cmp(&b.0.index)));
    out.push(sugg(
        "group",
        Severity::Low,
        "Pull pace",
        format!(
            "Pulls that gave forces at under half this key's usual pace ({median:.1}% a minute): {}. Check the kill order and cooldowns, or whether the pack is worth pulling.",
            slow.iter()
                .take(3)
                .map(|(p, r)| format!(
                    "pull {} at {} ({}, {:.1}%/min)",
                    p.index,
                    mmss(p.start_s),
                    mmss(p.end_s - p.start_s),
                    r
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        "Pace = enemy forces killed per minute of the pull. Compare with top runs: analyze --compare-top 5.".into(),
    ));
}
