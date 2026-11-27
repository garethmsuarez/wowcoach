//! M+ dispel advice: priority debuffs (`dispel_priority`) that expired or
//! killed someone while a dispeller was ready and in range, and players whose
//! priority dispels are slow. Reads the records `analysis/dispels.rs`
//! produces; players are only named when every check passed there.

use super::defensives::judged_deaths;
use super::{LIST_MAX, mmss, sugg};
use crate::analysis::*;

/// "Dispel speed" needs at least this many priority dispels.
const SPEED_MIN_SAMPLES: u32 = 3;
/// Debuffs up for less than this aren't judged: nobody dispels that fast
/// reliably (a quarter of priority dispels take 2 s or more).
const MIN_UP_S: f64 = 2.0;

/// Ready dispellers worth naming: in range, and a short dispel (not a group
/// cooldown like Revival or Mass Dispel).
fn ready_in_range(d: &DebuffRecord) -> Vec<&ReadyDispeller> {
    d.ready_dispellers
        .iter()
        .filter(|r| r.in_range == Some(true) && r.cd_s <= SHORT_DISPEL_CD_S)
        .collect()
}

/// Group "Dispels": undispelled priority debuffs with a dispeller ready.
pub(super) fn group(f: &FightReport, out: &mut Vec<Suggestion>) {
    let priority: Vec<&DebuffRecord> = f.debuffs.iter().filter(|d| d.priority).collect();
    let judged = judged_deaths(f);
    // A death with it up only counts if the death itself is judged (not a
    // late death in a wiping pull).
    let counts = |d: &DebuffRecord| match d.outcome {
        DebuffOutcome::Expired => true,
        DebuffOutcome::Died => judged
            .iter()
            .any(|x| x.player == d.player && (x.at_s - d.ended_s).abs() < 0.01),
        _ => false,
    };
    let mut missed: Vec<&DebuffRecord> = priority
        .iter()
        .copied()
        .filter(|d| {
            d.ended_s - d.applied_s >= MIN_UP_S && counts(d) && !ready_in_range(d).is_empty()
        })
        .collect();
    if missed.is_empty() {
        return;
    }
    let died = |d: &DebuffRecord| d.outcome == DebuffOutcome::Died;
    missed.sort_by(|a, b| {
        died(b)
            .cmp(&died(a))
            .then(a.applied_s.total_cmp(&b.applied_s))
            .then(a.player.cmp(&b.player))
    });
    let list = missed
        .iter()
        .take(LIST_MAX)
        .map(|d| {
            let how = if died(d) {
                format!("on {} when they died at {}", d.player, mmss(d.ended_s))
            } else {
                format!(
                    "on {} expired at {} after {:.0} s",
                    d.player,
                    mmss(d.ended_s),
                    d.ended_s - d.applied_s
                )
            };
            let who = ready_in_range(d)
                .iter()
                .map(|r| {
                    format!(
                        "{} ({}, {:.0} yd)",
                        r.player,
                        r.ability,
                        r.distance_yd.unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let pull = d.pull.map(|p| format!(" (pull {p})")).unwrap_or_default();
            format!("{} {how}{pull}; ready: {who}", d.name)
        })
        .collect::<Vec<_>>()
        .join(". ");
    let dispelled = priority
        .iter()
        .filter(|d| matches!(d.outcome, DebuffOutcome::Dispelled { .. }))
        .count();
    out.push(sugg(
        "group",
        if missed.iter().any(|d| died(d)) {
            Severity::High
        } else {
            Severity::Medium
        },
        "Dispels",
        format!(
            "{} priority debuff(s) went undispelled while someone could have dispelled them, worst first. {list}{}. {dispelled} of {} priority debuffs were dispelled this run.",
            missed.len(),
            if missed.len() > LIST_MAX { ". …" } else { "" },
            priority.len()
        ),
        "Ready = alive, with a dispel that removes the debuff's type, off cooldown before it ended, not busy dispelling something else, and within 40 yd by logged positions. A healer's own dispel always counts; others once used this run. Debuffs up under 2 s and group cooldowns (Revival, Mass Dispel) aren't counted.".into(),
    ));
}

/// Player "Dispel speed": median priority-dispel time over the bar.
pub(super) fn speed(f: &FightReport, p: &PlayerReport, bar_s: f64, out: &mut Vec<Suggestion>) {
    let Some(med) = p.dispel_latency_s else {
        return;
    };
    if p.priority_dispels < SPEED_MIN_SAMPLES || med <= bar_s {
        return;
    }
    let slowest = f
        .debuffs
        .iter()
        .filter(|d| d.priority)
        .filter_map(|d| match &d.outcome {
            DebuffOutcome::Dispelled { by, latency_s, .. } if *by == p.name => {
                Some((*latency_s, d))
            }
            _ => None,
        })
        .max_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(b.1.applied_s.total_cmp(&a.1.applied_s))
        });
    let worst = slowest
        .map(|(l, d)| {
            format!(
                " Slowest: {} on {} at {}, after {l:.1} s.",
                d.name,
                d.player,
                mmss(d.applied_s)
            )
        })
        .unwrap_or_default();
    out.push(sugg(
        &p.name,
        if med >= 2.0 * bar_s {
            Severity::Medium
        } else {
            Severity::Low
        },
        "Dispel speed",
        format!(
            "Priority debuffs took a median {med:.1} s to dispel ({} dispels).{worst} Dispel these as soon as they land: a focus or mouseover macro helps.",
            p.priority_dispels
        ),
        format!("Time from the debuff landing to its dispel. Flagged above {bar_s:.0} s (dispel_slow_s)."),
    ));
}
