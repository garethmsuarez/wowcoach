//! Turn metrics into actionable suggestions. Each rule is small and
//! independent so it's easy to add, tune or disable one.

use crate::analysis::*;
use crate::config::Config;
use crate::specs::Role;

fn mmss(s: f64) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn human(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.2}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.2}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1e3),
        n => n.to_string(),
    }
}

fn sugg(
    scope: &str,
    severity: Severity,
    category: &str,
    message: String,
    evidence: String,
) -> Suggestion {
    Suggestion {
        scope: scope.to_string(),
        severity,
        category: category.to_string(),
        message,
        evidence,
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Seconds a death costs the timer in this key: its affixes', else the config's.
fn death_penalty(f: &FightReport, cfg: &Config) -> f64 {
    f.death_penalty_s
        .unwrap_or(cfg.general.death_penalty_s as f64)
}

pub fn apply(f: &mut FightReport, cfg: &Config) {
    let mut out = vec![];
    group_rules(f, cfg, &mut out);
    player_rules(f, cfg, &mut out);
    out.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.scope.cmp(&b.scope)));
    f.suggestions = out;
}

fn group_rules(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let g = &cfg.general;
    let is_mplus = matches!(f.kind, FightKind::MythicPlus { .. });

    // --- Deaths
    let real_deaths: Vec<&DeathReport> = f.deaths.iter().filter(|d| !d.wipe_cascade).collect();
    let cascade = f.deaths.len() - real_deaths.len();
    if !real_deaths.is_empty() {
        let mut causes: Vec<(String, u32)> = vec![];
        for d in &real_deaths {
            let k = d.killing_blow.split(" (").next().unwrap_or("").to_string();
            match causes.iter_mut().find(|c| c.0 == k) {
                Some(c) => c.1 += 1,
                None => causes.push((k, 1)),
            }
        }
        causes.sort_by_key(|x| std::cmp::Reverse(x.1));
        let list = causes
            .iter()
            .take(3)
            .map(|(k, n)| format!("{k} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sev = if real_deaths.len() >= 3 {
            Severity::High
        } else {
            Severity::Medium
        };
        let mut msg = if cascade > 0 {
            format!(
                "{} death(s) before the wipe was called (plus {cascade} after it, which are ignored). Most common killing blows: {list}.",
                real_deaths.len()
            )
        } else {
            format!(
                "{} death(s). Most common killing blows: {list}.",
                real_deaths.len()
            )
        };
        if is_mplus {
            let pen = death_penalty(f, cfg);
            if pen > 0.0 {
                msg.push_str(&format!(
                    " At {pen:.0}s per death that's roughly {} added to the timer.",
                    mmss(f.deaths.len() as f64 * pen)
                ));
            } else {
                msg.push_str(
                    " Deaths cost no time in this key (affix), but they still slow the pull.",
                );
            }
        }
        out.push(sugg(
            "group",
            sev,
            "Deaths",
            msg,
            "See the death recaps below for what hit each player in their last seconds.".into(),
        ));
    }

    // --- Raid wipe analysis
    if let FightKind::Raid {
        success: false,
        boss_hp_pct,
        ..
    } = &f.kind
        && let Some(first) = f.deaths.first()
        && first.at_s < f.duration_s * g.early_death_frac
    {
        out.push(sugg(
            "group",
            Severity::High,
            "Wipe cause",
            format!(
                "First death came early ({} into a {} pull): {} to {}. Early deaths usually snowball into the wipe, so fix this first.",
                mmss(first.at_s),
                mmss(f.duration_s),
                first.player,
                first.killing_blow
            ),
            format!("Boss ended at {:.1}% HP.", boss_hp_pct.unwrap_or(100.0)),
        ));
    }

    // --- Interrupts
    let mut missed: Vec<&EnemyCastReport> =
        f.enemy_casts.iter().filter(|c| c.succeeded > 0).collect();
    // Priority casts first, so they aren't buried behind filler.
    missed.sort_by_key(|c| !c.priority);
    if !missed.is_empty() {
        let total_missed: u32 = missed.iter().map(|c| c.succeeded).sum();
        let pri = missed.iter().any(|c| c.priority);
        let list = missed
            .iter()
            .take(5)
            .map(|c| {
                format!(
                    "{} ({} went off, {} kicked{})",
                    c.name,
                    c.succeeded,
                    c.interrupted,
                    if c.priority { ", PRIORITY" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(sugg(
            "group",
            if pri || total_missed >= 5 { Severity::High } else { Severity::Medium },
            "Interrupts",
            format!("{total_missed} interruptible enemy casts went through. Assign a kick rotation for these: {list}."),
            "A spell is treated as interruptible if anyone interrupted it at least once in this log, or if it's in the priority_interrupt config.".into(),
        ));
        let lazy: Vec<&str> = f
            .players
            .iter()
            .filter(|p| p.role != Role::Healer && p.interrupts == 0)
            .map(|p| p.name.as_str())
            .collect();
        if !lazy.is_empty() && is_mplus {
            out.push(sugg(
                "group",
                Severity::Medium,
                "Interrupts",
                format!(
                    "Kicks were missed while {} interrupted nothing. Spread the kick assignments.",
                    lazy.join(", ")
                ),
                String::new(),
            ));
        }
    }

    // --- Priority kicks (M+): who could have kicked what went off
    if is_mplus {
        priority_kicks(f, out);
        priority_stops(f, out);
        defensives::apply(f, cfg, out);
        dispels::group(f, out);
    }

    // --- Avoidable damage
    let configured: Vec<&EnemySpellReport> = f
        .enemy_damage
        .iter()
        .filter(|s| s.configured_avoidable)
        .collect();
    if !configured.is_empty() {
        let list = configured
            .iter()
            .take(5)
            .map(|s| format!("{} ({} over {} hits)", s.name, human(s.total), s.hits))
            .collect::<Vec<_>>()
            .join("; ");
        out.push(sugg(
            "group",
            Severity::High,
            "Avoidable damage",
            format!("The group took avoidable damage from: {list}."),
            "These spells are listed as avoidable in your config.".into(),
        ));
    }
    let likely: Vec<&EnemySpellReport> = f
        .enemy_damage
        .iter()
        .filter(|s| s.likely_avoidable)
        .take(4)
        .collect();
    if !likely.is_empty() {
        let list = likely
            .iter()
            .map(|s| {
                let top = s
                    .per_player
                    .iter()
                    .take(2)
                    .map(|p| format!("{} {}×", p.0, p.2))
                    .collect::<Vec<_>>()
                    .join(", ");
                let why = if s.non_tank_hit_frac <= 0.6 {
                    format!("only {:.0}% of non-tanks hit", s.non_tank_hit_frac * 100.0)
                } else {
                    format!(
                        "up to {} hits on one player vs a median of {:.0}",
                        s.max_hits_per_player, s.median_hits_per_player
                    )
                };
                format!("{} ({}, {why}; most: {top})", s.name, human(s.total))
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(sugg(
            "group",
            Severity::Low,
            "Possibly avoidable",
            format!("These spells hit some players far more than others, which usually means they can be dodged. Worth checking: {list}."),
            "This is a heuristic based on uneven hit distribution. Targeted debuffs can also trigger it; add confirmed spells to [[avoidable]].".into(),
        ));
    }

    // --- Bloodlust
    if matches!(f.kind, FightKind::Raid { success: true, .. })
        && f.duration_s > 90.0
        && f.lust_at_s.is_empty()
    {
        out.push(sugg(
            "group",
            Severity::Medium,
            "Cooldowns",
            "No Bloodlust/Heroism was detected on this pull.".into(),
            "Detected via cast IDs listed in lust_spell_ids.".into(),
        ));
    }
    if let FightKind::Raid { success: true, .. } = f.kind
        && let Some(l) = f.lust_at_s.first()
        && *l > f.duration_s * 0.5
        && f.duration_s < 300.0
    {
        out.push(sugg(
            "group",
            Severity::Low,
            "Cooldowns",
            format!("Lust went out at {} on a {} kill. On short fights, lusting on pull usually gives more value.", mmss(*l), mmss(f.duration_s)),
            String::new(),
        ));
    }

    // --- M+ pacing
    if let FightKind::MythicPlus {
        success, time_s, ..
    } = &f.kind
    {
        let down_frac = if f.duration_s > 0.0 {
            f.downtime_s / f.duration_s
        } else {
            0.0
        };
        if down_frac > 0.25 && f.duration_s > 300.0 {
            let mut gaps: Vec<(f64, f64)> = f
                .pulls
                .windows(2)
                .map(|w| (w[0].end_s, w[1].start_s - w[0].end_s))
                .collect();
            gaps.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let list = gaps
                .iter()
                .take(3)
                .map(|(at, len)| format!("{} gap at {}", mmss(*len), mmss(*at)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(sugg(
                "group",
                if !success { Severity::High } else { Severity::Medium },
                "Pacing",
                format!(
                    "{:.0}% of the run ({}) was spent out of combat. Chain pulls, pre-position, and drink less. Longest gaps: {list}.",
                    down_frac * 100.0,
                    mmss(f.downtime_s)
                ),
                format!("{} pulls detected (a gap of {}s or more with no combat counts as a break).", f.pulls.len(), g.pull_gap_ms / 1000),
            ));
        }
        let bad: Vec<&PullReport> = f.pulls.iter().filter(|p| p.deaths >= 3).collect();
        if !bad.is_empty() {
            let list = bad
                .iter()
                .map(|p| {
                    format!(
                        "pull {} at {} ({} deaths{})",
                        p.index,
                        mmss(p.start_s),
                        p.deaths,
                        p.boss
                            .as_ref()
                            .map(|b| format!(", {b}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            out.push(sugg(
                "group",
                Severity::High,
                "Pull planning",
                format!("Near-wipe or wipe pulls: {list}. Plan defensives and stops for these packs, or pull fewer mobs at once."),
                String::new(),
            ));
        }
        if !success {
            out.push(sugg(
                "group",
                Severity::Medium,
                "Timer",
                format!("Key not timed ({}).", mmss(*time_s)),
                if death_penalty(f, cfg) > 0.0 {
                    "Biggest levers, in order: deaths, downtime, then damage output.".into()
                } else {
                    "Biggest levers, in order: downtime, then damage output (deaths cost no time in this key).".into()
                },
            ));
        }
    }
}

/// How many events a message lists before summarising.
const LIST_MAX: usize = 4;

fn priority_kicks(f: &FightReport, out: &mut Vec<Suggestion>) {
    let went_off: Vec<&CastRecord> = f
        .casts
        .iter()
        .filter(|c| c.priority && c.outcome == CastOutcome::Completed)
        .collect();
    if went_off.is_empty() {
        return;
    }
    let ready_in_range = |c: &CastRecord| -> Vec<String> {
        c.ready_kickers
            .iter()
            .filter(|k| k.in_range == Some(true))
            .map(|k| {
                format!(
                    "{} ({}, {:.0} yd)",
                    k.player,
                    k.ability,
                    k.distance_yd.unwrap_or_default()
                )
            })
            .collect()
    };
    let covered: Vec<(&CastRecord, Vec<String>)> = went_off
        .iter()
        .map(|c| (*c, ready_in_range(c)))
        .filter(|(_, r)| !r.is_empty())
        .collect();
    let uncovered = went_off.len() - covered.len();
    let mut msg = format!(
        "{} priority cast(s) went off. For {} of them someone had a kick ready and in range",
        went_off.len(),
        covered.len()
    );
    if covered.is_empty() {
        msg.push('.');
    } else {
        // Who was most often ready, then one example; per-event detail is in
        // each player's section and the priority casts table.
        let mut times: Vec<(String, usize)> = vec![];
        for c in &went_off {
            for k in c.ready_kickers.iter().filter(|k| k.in_range == Some(true)) {
                match times.iter_mut().find(|t| t.0 == k.player) {
                    Some(t) => t.1 += 1,
                    None => times.push((k.player.clone(), 1)),
                }
            }
        }
        times.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let (c, who) = &covered[0];
        msg.push_str(&format!(
            ". Most often ready: {}. First: {} at {}{}, with {} ready.",
            times
                .iter()
                .map(|(p, n)| format!("{p} ×{n}"))
                .collect::<Vec<_>>()
                .join(", "),
            c.name,
            mmss(c.end_s),
            c.pull.map(|p| format!(" (pull {p})")).unwrap_or_default(),
            who.join(", ")
        ));
    }
    if uncovered > 0 {
        msg.push_str(&format!(
            " For {uncovered}, nobody's kick was ready: those packs need more kickers or a stun."
        ));
    }
    out.push(sugg(
        "group",
        if covered.is_empty() { Severity::Medium } else { Severity::High },
        "Priority kicks",
        msg,
        "Ready = alive, with an interrupt off cooldown before the cast finished, and within its range by logged positions. The spec's main interrupt always counts; others once used this run.".into(),
    ));
}

/// Priority casts that can also be stopped by crowd control, that went off
/// while someone had a stop ready and in range.
fn priority_stops(f: &FightReport, out: &mut Vec<Suggestion>) {
    let missed: Vec<(&CastRecord, Vec<String>)> = f
        .casts
        .iter()
        .filter(|c| c.priority && c.outcome == CastOutcome::Completed)
        .map(|c| {
            let who = c
                .ready_stoppers
                .iter()
                .filter(|k| k.in_range == Some(true))
                .map(|k| {
                    format!(
                        "{} ({}, {:.0} yd)",
                        k.player,
                        k.ability,
                        k.distance_yd.unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>();
            (c, who)
        })
        .filter(|(_, who)| !who.is_empty())
        .collect();
    if missed.is_empty() {
        return;
    }
    let list = missed
        .iter()
        .take(LIST_MAX)
        .map(|(c, who)| format!("{} at {}: {}", c.name, mmss(c.end_s), who.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    out.push(sugg(
        "group",
        Severity::Medium,
        "Stops",
        format!(
            "{} priority cast(s) that crowd control also stops went off while a stop was ready: {list}{}. When kicks are down, a stun or knock does the job.",
            missed.len(),
            if missed.len() > LIST_MAX { "; …" } else { "" }
        ),
        "A stop counts once its player has used it this run. Ranges without a listed value are taken as 8 yd.".into(),
    ));
}

fn player_rules(f: &FightReport, cfg: &Config, out: &mut Vec<Suggestion>) {
    let g = &cfg.general;
    // Activity is an estimate that reads low for everyone, so judge each
    // player against the group rather than an absolute bar alone.
    let act_med = median(
        f.players
            .iter()
            .filter(|p| p.role != Role::Tank && p.casts > 0)
            .map(|p| p.activity_pct)
            .collect(),
    );
    let act_bar = g.activity_warn.min(act_med - 0.10);
    let dps_vals: Vec<f64> = f
        .players
        .iter()
        .filter(|p| p.role == Role::Dps)
        .map(|p| p.dps)
        .collect();
    let n_dps = dps_vals.len();
    let dps_med = median(dps_vals);
    let avoid_med = median(
        f.players
            .iter()
            .filter(|p| p.role != Role::Tank)
            .map(|p| p.avoidable_taken as f64)
            .collect(),
    );
    let likely_med = median(
        f.players
            .iter()
            .filter(|p| p.role != Role::Tank)
            .map(|p| p.likely_avoidable_taken as f64)
            .collect(),
    );

    for p in &f.players {
        let me = p.name.as_str();
        // deaths (one suggestion per player, grouped)
        let mine: Vec<&DeathReport> = f
            .deaths
            .iter()
            .filter(|d| d.player == p.name && !d.wipe_cascade)
            .collect();
        if let Some(last) = mine.last() {
            let avoidable_deaths = mine
                .iter()
                .filter(|d| cfg.avoidable_note(d.killing_spell_id).is_some())
                .count();
            let mut msg = if mine.len() == 1 {
                format!("Died at {} to {}.", mmss(last.at_s), last.killing_blow)
            } else {
                let times = mine
                    .iter()
                    .map(|d| mmss(d.at_s))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut causes: Vec<(String, u32)> = vec![];
                for d in &mine {
                    let k = d.killing_blow.split(" (").next().unwrap_or("").to_string();
                    match causes.iter_mut().find(|c| c.0 == k) {
                        Some(c) => c.1 += 1,
                        None => causes.push((k, 1)),
                    }
                }
                causes.sort_by_key(|x| std::cmp::Reverse(x.1));
                let cs = causes
                    .iter()
                    .map(|(k, n)| format!("{k} ×{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Died {} times ({times}). Killing blows: {cs}.", mine.len())
            };
            if avoidable_deaths > 0 {
                let note = mine
                    .iter()
                    .find_map(|d| cfg.avoidable_note(d.killing_spell_id))
                    .map(|a| {
                        if a.note.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", a.note)
                        }
                    })
                    .unwrap_or_default();
                if mine.len() == 1 {
                    msg.push_str(&format!(" That's an avoidable mechanic{note}."));
                } else {
                    msg.push_str(&format!(
                        " {avoidable_deaths} of these were to avoidable mechanics{note}."
                    ));
                }
            } else if mine
                .iter()
                .all(|d| !d.healthstone_used_before && !d.potion_used_before)
            {
                msg.push_str(" No healthstone or potion was used before dying. Use a defensive or healthstone earlier in the damage window.");
            }
            let with_debuffs: Vec<String> = mine
                .iter()
                .filter(|d| !d.priority_debuffs.is_empty())
                .map(|d| format!("{} at {}", d.priority_debuffs.join(" and "), mmss(d.at_s)))
                .collect();
            if !with_debuffs.is_empty() {
                msg.push_str(&format!(
                    " Priority dispels up when you died: {}.",
                    with_debuffs.join(", ")
                ));
            }
            let recap = last
                .recap
                .iter()
                .map(|r| format!("{} {} {}", mmss(r.at_s), r.spell, human(r.amount)))
                .collect::<Vec<_>>()
                .join(" → ");
            let sev = if avoidable_deaths > 0 || mine.len() >= 3 {
                Severity::High
            } else {
                Severity::Medium
            };
            out.push(sugg(
                me,
                sev,
                "Death",
                msg,
                format!(
                    "Last death, final hits: {recap}. Damage taken in the final 5s: {}.",
                    human(last.damage_last_5s)
                ),
            ));
        }
        // configured avoidable
        let avoid_rank = f
            .players
            .iter()
            .filter(|o| o.role != Role::Tank && o.avoidable_taken > p.avoidable_taken)
            .count();
        let top_quartile = avoid_rank < (f.players.len() / 4).max(1);
        if p.role != Role::Tank
            && p.avoidable_taken > 0
            && top_quartile
            && (p.avoidable_taken as f64) > 2.0 * avoid_med.max(1.0)
        {
            out.push(sugg(
                me,
                if (p.avoidable_taken as f64) > 4.0 * avoid_med.max(1.0) {
                    Severity::High
                } else {
                    Severity::Medium
                },
                "Avoidable damage",
                format!(
                    "Took {} avoidable damage, more than the group median ({}).",
                    human(p.avoidable_taken),
                    human(avoid_med as u64)
                ),
                top_taken(p),
            ));
        }
        if p.role != Role::Tank
            && p.likely_avoidable_taken > 0
            && p.likely_avoidable_taken as f64 >= 0.05 * p.damage_taken as f64
            && (p.likely_avoidable_taken as f64) > 2.0 * likely_med.max(1.0)
        {
            out.push(sugg(
                me,
                Severity::Low,
                "Possibly avoidable",
                format!(
                    "Took {} from spells that most of the group avoided (group median {}).",
                    human(p.likely_avoidable_taken),
                    human(likely_med as u64)
                ),
                top_taken(p),
            ));
        }
        // activity
        if p.role != Role::Tank && f.duration_s >= 30.0 && p.casts > 0 && p.activity_pct < act_bar {
            out.push(sugg(
                me,
                if p.activity_pct < act_bar - 0.15 { Severity::Medium } else { Severity::Low },
                "Activity",
                format!(
                    "Estimated casting activity was {:.0}% (group median {:.0}%). Look for gaps in your GCD usage: keep casting while moving, and pre-cast before pulls.",
                    p.activity_pct * 100.0,
                    act_med * 100.0
                ),
                "This estimate comes from cast events plus one GCD each. Channelled spells are undercounted, so compare it against your own previous logs.".into(),
            ));
        }
        // output vs group
        if p.role == Role::Dps && n_dps >= 3 && dps_med > 0.0 && p.dps < g.dps_warn_ratio * dps_med
        {
            let mut why = vec![];
            if p.deaths > 0 {
                why.push(format!("{} death(s)", p.deaths));
            }
            if p.activity_pct < g.activity_warn {
                why.push(format!("{:.0}% activity", p.activity_pct * 100.0));
            }
            out.push(sugg(
                me,
                Severity::Low,
                "Output",
                format!(
                    "DPS ({}) was {:.0}% of the group DPS median ({}).{}",
                    human(p.dps as u64),
                    100.0 * p.dps / dps_med,
                    human(dps_med as u64),
                    if why.is_empty() { String::new() } else { format!(" Likely contributors: {}.", why.join(", ")) }
                ),
                "Spec, item level and assignments (soaks, add duty) all affect this, so read it as a pointer, not a verdict.".into(),
            ));
        }
        if let Some(r) = p.wcl_rank_percent
            && r < 25.0
        {
            out.push(sugg(
                me,
                Severity::Low,
                "Benchmark",
                format!("Warcraft Logs ranks this parse at {r:.0} for the spec on this encounter."),
                String::new(),
            ));
        }
        // healers
        if p.role == Role::Healer && p.overheal_pct > g.overheal_warn && p.healing_done > 0 {
            out.push(sugg(
                me,
                Severity::Low,
                "Healing efficiency",
                format!("{:.0}% overhealing. Hold big heals for damage windows, and spend the downtime on damage or mana.", p.overheal_pct * 100.0),
                String::new(),
            ));
        }
        // priority kicks missed and kicks wasted (M+)
        if matches!(f.kind, FightKind::MythicPlus { .. }) {
            let missed = missed_priority(&f.casts, &p.name);
            if !missed.is_empty() {
                // Casts of the same spell finishing together (several mobs) read as one.
                let mut events: Vec<(String, String, usize)> = vec![];
                for (c, _) in &missed {
                    let at = mmss(c.end_s);
                    match events.iter_mut().find(|e| e.0 == c.name && e.1 == at) {
                        Some(e) => e.2 += 1,
                        None => events.push((c.name.clone(), at, 1)),
                    }
                }
                let list = events
                    .iter()
                    .take(LIST_MAX)
                    .map(|(n, at, k)| {
                        if *k > 1 {
                            format!("{n} at {at} (×{k})")
                        } else {
                            format!("{n} at {at}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ability = &missed[0].1.ability;
                out.push(sugg(
                    me,
                    if missed.len() >= 3 {
                        Severity::High
                    } else {
                        Severity::Medium
                    },
                    "Priority kicks",
                    format!(
                        "{} ready and in range for {} priority cast(s) that went off: {list}{}.",
                        ability,
                        missed.len(),
                        if events.len() > LIST_MAX { ", …" } else { "" }
                    ),
                    format!(
                        "Distances when the casts finished: {}.",
                        missed
                            .iter()
                            .take(LIST_MAX)
                            .map(|(_, k)| format!("{:.0} yd", k.distance_yd.unwrap_or_default()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            let wasted = wasted_kicks(&f.casts, &p.name);
            if !wasted.is_empty() {
                let (k, pc) = wasted[0];
                out.push(sugg(
                    me,
                    if wasted.len() >= 2 { Severity::Medium } else { Severity::Low },
                    "Wasted kicks",
                    format!(
                        "Kicked {} at {} while {} was already casting; it went off at {} with your kick on cooldown{}. Save the kick for the priority cast.",
                        k.name,
                        mmss(k.end_s),
                        pc.name,
                        mmss(pc.end_s),
                        if wasted.len() > 1 { format!(" ({} times this run)", wasted.len()) } else { String::new() }
                    ),
                    String::new(),
                ));
            }
        }
        if matches!(f.kind, FightKind::MythicPlus { .. }) {
            dispels::speed(f, p, g.dispel_slow_s, out);
        }
        // interrupts in M+ (unless the specific "Priority kicks" advice covers it)
        if matches!(f.kind, FightKind::MythicPlus { .. })
            && p.role != Role::Healer
            && p.interrupts == 0
            && missed_priority(&f.casts, &p.name).is_empty()
            && f.enemy_casts.iter().any(|c| c.succeeded > 0)
        {
            out.push(sugg(me, Severity::Medium, "Interrupts", "No interrupts this run, even though casts were getting through. Put your kick on your rotation.".into(), String::new()));
        }
    }
}

fn top_taken(p: &PlayerReport) -> String {
    p.top_damage_taken
        .iter()
        .take(3)
        .map(|(n, a, h)| format!("{n} {} ({h} hits)", human(*a)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn apply_progression(p: &mut ProgressionReport) {
    let mut out = vec![];
    if let Some((spell, n)) = p.first_death_causes.first()
        && *n >= 2
        && (*n as f64) / (p.attempts as f64) >= 0.3
    {
        out.push(sugg(
            "group",
            Severity::High,
            "Progression",
            format!(
                "{spell} caused the first death in {n} of {} pulls. Solve it before anything else.",
                p.attempts
            ),
            String::new(),
        ));
    }
    if let Some((who, n)) = p.frequent_deaths.first()
        && *n >= 3
        && *n as f64 >= 0.5 * p.attempts as f64
    {
        out.push(sugg(
            who,
            Severity::Medium,
            "Progression",
            format!("Died before the wipe in {n} of {} pulls. Review this player's death recaps for a repeated pattern.", p.attempts),
            String::new(),
        ));
    }
    if p.kills == 0
        && let Some(b) = p.best_hp_pct
    {
        out.push(sugg(
            "group",
            Severity::Low,
            "Progression",
            format!(
                "{} pulls so far, best at {b:.1}%. Average pull length: {}.",
                p.attempts,
                mmss(p.avg_duration_s)
            ),
            String::new(),
        ));
    }
    p.suggestions = out;
}

mod defensives;
mod dispels;
pub mod kickplan;

#[cfg(test)]
mod tests;
