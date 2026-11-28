//! Self-contained HTML report.

use crate::analysis::*;
use crate::rules::human;
use serde::Serialize;
use std::fmt::Write;

#[derive(Serialize)]
pub struct Report {
    pub source: String,
    /// Season pack the spell knowledge came from ("midnight-s2"), if any.
    pub season_pack: Option<String>,
    pub fights: Vec<FightReport>,
    pub progression: Vec<ProgressionReport>,
    /// Suggested kick assignments per M+ dungeon, across its keys.
    pub kick_plans: Vec<crate::rules::kickplan::KickPlan>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn mmss(s: f64) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

const CSS: &str = r#"
:root{--bg:#0f1115;--card:#171a21;--line:#262b36;--fg:#e6e8ee;--mut:#8b93a7;--hi:#ff6b6b;--md:#f6b73c;--lo:#5fb3f9;--ok:#4cd18a}
*{box-sizing:border-box}body{margin:0;font:14px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;background:var(--bg);color:var(--fg)}
main{max-width:1200px;margin:0 auto;padding:24px}h1{font-size:22px;margin:0 0 4px}h2{font-size:18px;margin:0}h3{font-size:14px;margin:18px 0 8px;color:var(--mut);text-transform:uppercase;letter-spacing:.06em}
.sub{color:var(--mut)}.card{background:var(--card);border:1px solid var(--line);border-radius:10px;margin:18px 0;overflow:hidden}
details>summary{cursor:pointer;padding:14px 18px;list-style:none;display:flex;gap:12px;align-items:center;flex-wrap:wrap}details>summary::-webkit-details-marker{display:none}
.body{padding:0 18px 18px}.chip{font-size:12px;padding:2px 8px;border-radius:99px;border:1px solid var(--line);color:var(--mut)}
.ok{color:var(--ok);border-color:#2a5a40}.bad{color:var(--hi);border-color:#5a2a2a}
table{width:100%;border-collapse:collapse;font-size:13px}th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);vertical-align:top}th{color:var(--mut);font-weight:500}td.n{text-align:right;font-variant-numeric:tabular-nums}
.s{border-left:3px solid var(--line);padding:8px 12px;margin:6px 0;background:#1b1f27;border-radius:0 6px 6px 0}.s.High{border-color:var(--hi)}.s.Medium{border-color:var(--md)}.s.Low{border-color:var(--lo)}
.s .ev{color:var(--mut);font-size:12px;margin-top:2px}.tag{font-size:11px;font-weight:600;margin-right:6px}.High .tag{color:var(--hi)}.Medium .tag{color:var(--md)}.Low .tag{color:var(--lo)}
.bar{height:6px;background:#2a3040;border-radius:3px;min-width:60px}.bar>i{display:block;height:100%;border-radius:3px;background:var(--lo)}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(340px,1fr));gap:16px}
.timeline{position:relative;height:26px;background:#1b1f27;border-radius:4px;margin:6px 0 2px}.timeline i{position:absolute;top:4px;height:18px;background:#3a6fb0;border-radius:2px}.timeline i.d{background:var(--hi)}
.timeline b{position:absolute;top:0;width:2px;height:26px;background:var(--md)}
"#;

fn suggestions_html(s: &[Suggestion], out: &mut String) {
    let groups: Vec<&Suggestion> = s.iter().filter(|x| x.scope == "group").collect();
    if !groups.is_empty() {
        out.push_str("<h3>Group</h3>");
        for x in groups {
            one(x, out, false);
        }
    }
    let mut names: Vec<&str> = s
        .iter()
        .filter(|x| x.scope != "group")
        .map(|x| x.scope.as_str())
        .collect();
    names.sort();
    names.dedup();
    if !names.is_empty() {
        out.push_str("<h3>Individual</h3><div class=grid>");
        for n in names {
            let _ = write!(out, "<div><b>{}</b>", esc(n));
            for x in s.iter().filter(|x| x.scope == n) {
                one(x, out, true);
            }
            out.push_str("</div>");
        }
        out.push_str("</div>");
    }
}

fn one(x: &Suggestion, out: &mut String, _player: bool) {
    let _ = write!(
        out,
        "<div class='s {:?}'><span class=tag>{:?} · {}</span>{}{}</div>",
        x.severity,
        x.severity,
        esc(&x.category),
        esc(&x.message),
        if x.evidence.is_empty() {
            String::new()
        } else {
            format!("<div class=ev>{}</div>", esc(&x.evidence))
        }
    );
}

/// Suggested kick assignments for one dungeon.
fn kick_plan_html(k: &crate::rules::kickplan::KickPlan, o: &mut String) {
    if k.packs.is_empty() {
        return;
    }
    let _ = write!(
        o,
        "<div class=card><details open><summary><h2>Kick plan: {}</h2><span class=chip>{} key{}</span><span class=chip>{} packs</span></summary><div class=body><div class=sub>For {}. Each pack is the set of enemies casting priority spells in a pull. Must-kicks are assigned first; spells marked “with spare kicks” only get kicks left over; rates are casts per minute while that pack was in combat. Kickers can do 60 ÷ cooldown kicks a minute, with a 25% margin, preferring whoever already kicked that spell, then whoever was usually in range. A starting point, not a rule.</div><table><tr><th>Pack</th><th>Priority casts</th><th>Suggested kickers</th></tr>",
        esc(&k.dungeon),
        k.keys,
        if k.keys == 1 { "" } else { "s" },
        k.packs.len(),
        esc(&k.group.join(", "))
    );
    for p in &k.packs {
        let spells = p
            .spells
            .iter()
            .map(|s| {
                format!(
                    "{}{} <span class=sub>{:.1}/min · {} kicked, {} stopped, {}</span>",
                    esc(&s.spell),
                    if s.tier == 2 {
                        " <span class=sub>(with spare kicks)</span>"
                    } else {
                        ""
                    },
                    s.casts_per_min,
                    s.kicked,
                    s.stopped,
                    if s.went_off > 0 {
                        format!("<b class=bad>{} went off</b>", s.went_off)
                    } else {
                        "0 went off".into()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("<br>");
        let mut who = p
            .assignments
            .iter()
            .map(|a| {
                format!(
                    "{} ({}): {}",
                    esc(&a.player),
                    esc(&a.ability),
                    esc(&a.spells.join(", "))
                )
            })
            .collect::<Vec<_>>()
            .join("<br>");
        if !p.uncovered.is_empty() {
            let _ = write!(
                who,
                "<br><b class=bad>Not enough kicks for {}: plan a stun or another kicker</b>",
                esc(&p.uncovered.join(", "))
            );
        }
        let _ = write!(
            o,
            "<tr><td>{}<div class=sub>{} pull{}</div></td><td>{}</td><td>{}</td></tr>",
            esc(&p.name),
            p.pulls,
            if p.pulls == 1 { "" } else { "s" },
            spells,
            who
        );
    }
    o.push_str("</table></div></details></div>");
}

/// Every priority cast of a key and how it ended, grouped by pull.
fn priority_casts_html(f: &FightReport, o: &mut String) {
    let casts: Vec<&CastRecord> = f.casts.iter().filter(|c| c.priority).collect();
    if casts.is_empty() {
        return;
    }
    let off = casts
        .iter()
        .filter(|c| c.outcome == CastOutcome::Completed)
        .count();
    let _ = write!(
        o,
        "<h3>Priority casts</h3><div class=sub>{} cast · {} went off</div><table><tr><th>Time</th><th>Pull</th><th>Spell</th><th>Caster</th><th>Outcome</th></tr>",
        casts.len(),
        off
    );
    for c in casts {
        let outcome = match &c.outcome {
            CastOutcome::Completed => {
                let ready: Vec<String> = c
                    .ready_kickers
                    .iter()
                    .map(|k| {
                        let d = k
                            .distance_yd
                            .map(|d| format!("{d:.0} yd"))
                            .unwrap_or_else(|| "distance unknown".into());
                        let range = if k.in_range == Some(false) {
                            ", out of range"
                        } else {
                            ""
                        };
                        format!("{} ({}, {d}{range})", esc(&k.player), esc(&k.ability))
                    })
                    .collect();
                let stops: Vec<String> = c
                    .ready_stoppers
                    .iter()
                    .filter(|k| k.in_range == Some(true))
                    .map(|k| format!("{} ({})", esc(&k.player), esc(&k.ability)))
                    .collect();
                format!(
                    "<b class=bad>went off</b>{}{}",
                    if ready.is_empty() {
                        " · no kick ready".to_string()
                    } else {
                        format!(" · ready: {}", ready.join(", "))
                    },
                    if stops.is_empty() {
                        String::new()
                    } else {
                        format!(" · stops ready: {}", stops.join(", "))
                    }
                )
            }
            CastOutcome::Kicked {
                by,
                spell,
                latency_s,
            } => {
                format!("kicked by {} ({}, {latency_s:.1}s in)", esc(by), esc(spell))
            }
            CastOutcome::Stopped { by, ability, .. } => {
                format!("stopped by {} ({})", esc(by), esc(ability))
            }
            CastOutcome::CasterDied => "caster killed".into(),
            CastOutcome::Cancelled => "<span class=sub>cancelled</span>".into(),
        };
        let _ = write!(
            o,
            "<tr><td>{}</td><td class=n>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            mmss(c.end_s),
            c.pull.map(|p| p.to_string()).unwrap_or_default(),
            esc(&c.name),
            esc(&c.caster),
            outcome
        );
    }
    o.push_str("</table>");
}

/// One row per M+ pull with something defensive to look at: damage spikes
/// (and group defensives ready but unused), tank busters taken bare, deaths
/// with a personal defensive ready, and lust.
fn defensives_html(f: &FightReport, o: &mut String) {
    let rows: Vec<String> = f
        .pulls
        .iter()
        .filter_map(|p| {
            let i = Some(p.index);
            let spikes: Vec<&Spike> = f.spikes.iter().filter(|s| s.pull == i).collect();
            let deaths: Vec<&DeathReport> = f
                .deaths
                .iter()
                .filter(|d| d.pull == i && !d.defensives_ready.is_empty())
                .collect();
            if spikes.is_empty() && p.tank_hits_unmitigated == 0 && deaths.is_empty() && !p.lust_used {
                return None;
            }
            let spike_cell = spikes
                .iter()
                .map(|s| {
                    let answer = if !s.group_used.is_empty() {
                        format!(
                            "used {}",
                            s.group_used
                                .iter()
                                .map(|g| esc(&g.ability))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else if s.top.first().is_some_and(|t| t.summoned_by.is_some()) {
                        format!("<b class=bad>{} up</b>", esc(&s.top[0].source))
                    } else if !s.group_ready.is_empty() {
                        format!(
                            "<b class=bad>ready: {}</b>",
                            s.group_ready
                                .iter()
                                .map(|r| format!("{} ({})", esc(&r.ability), esc(&r.player)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        "none ready".into()
                    };
                    format!(
                        "{} {:.0}% · {} · {answer}",
                        mmss(s.start_s),
                        s.peak_frac * 100.0,
                        s.top.first().map(|t| esc(&t.name)).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("<br>");
            let tank = if p.tank_hits > 0 {
                let cls = if p.tank_hits_unmitigated > 0 { " class=bad" } else { "" };
                format!("<span{cls}>{} of {} bare</span>", p.tank_hits_unmitigated, p.tank_hits)
            } else {
                String::new()
            };
            let death_cell = deaths
                .iter()
                .map(|d| {
                    format!(
                        "{} {} ({})",
                        mmss(d.at_s),
                        esc(&d.player),
                        d.defensives_ready
                            .iter()
                            .map(|r| esc(&r.ability))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("<br>");
            let lust = match (p.lust_used, p.lust_ready_at_start) {
                (true, _) => "used",
                (false, true) => "ready",
                _ => "",
            };
            Some(format!(
                "<tr><td class=n>{}</td><td>{}</td><td>{spike_cell}</td><td>{tank}</td><td>{death_cell}</td><td>{lust}</td></tr>",
                p.index,
                p.boss.as_deref().map(esc).unwrap_or_default()
            ))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    o.push_str("<h3>Defensives by pull</h3><div class=sub>Spikes: the group losing half its HP within a few seconds, and the answer · tank busters with no mitigation up · deaths with a personal defensive ready</div><table><tr><th>Pull</th><th>Boss</th><th>Spikes</th><th>Tank busters</th><th>Died with a defensive ready</th><th>Lust</th></tr>");
    for r in rows {
        o.push_str(&r);
    }
    o.push_str("</table>");
}

/// One row per M+ pull with priority debuffs (`dispel_priority`): how many
/// were dispelled and how fast, and the ones that weren't, with who was ready.
fn dispels_html(f: &FightReport, o: &mut String) {
    let rows: Vec<String> = f
        .pulls
        .iter()
        .filter(|p| p.priority_debuffs > 0)
        .map(|p| {
            let i = Some(p.index);
            let mine: Vec<&DebuffRecord> =
                f.debuffs.iter().filter(|d| d.priority && d.pull == i).collect();
            let mut lat: Vec<f64> = mine
                .iter()
                .filter_map(|d| match &d.outcome {
                    DebuffOutcome::Dispelled { latency_s, .. } => Some(*latency_s),
                    _ => None,
                })
                .collect();
            lat.sort_by(f64::total_cmp);
            let med = match lat.len() {
                0 => String::new(),
                n if n % 2 == 1 => format!("{:.1}s", lat[n / 2]),
                n => format!("{:.1}s", (lat[n / 2 - 1] + lat[n / 2]) / 2.0),
            };
            let missed = mine
                .iter()
                .filter(|d| !matches!(d.outcome, DebuffOutcome::Dispelled { .. }))
                .map(|d| {
                    let how = match d.outcome {
                        DebuffOutcome::Died => "died with it",
                        DebuffOutcome::Lasted => "still up at the end",
                        _ => "expired",
                    };
                    let ready: Vec<String> = d
                        .ready_dispellers
                        .iter()
                        .filter(|r| r.in_range == Some(true) && r.cd_s <= SHORT_DISPEL_CD_S)
                        .map(|r| format!("{} ({})", esc(&r.player), esc(&r.ability)))
                        .collect();
                    let answer = if ready.is_empty() {
                        "none ready".to_string()
                    } else {
                        format!("<b class=bad>ready: {}</b>", ready.join(", "))
                    };
                    format!(
                        "{} {} on {}, {how} · {answer}",
                        mmss(d.applied_s),
                        esc(&d.name),
                        esc(&d.player)
                    )
                })
                .collect::<Vec<_>>()
                .join("<br>");
            let cls = if p.priority_dispelled < p.priority_debuffs {
                " class=bad"
            } else {
                ""
            };
            format!(
                "<tr><td class=n>{}</td><td>{}</td><td class=n><span{cls}>{} of {}</span></td><td class=n>{med}</td><td>{missed}</td></tr>",
                p.index,
                p.boss.as_deref().map(esc).unwrap_or_default(),
                p.priority_dispelled,
                p.priority_debuffs
            )
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    o.push_str("<h3>Dispels by pull</h3><div class=sub>Priority debuffs (dispel_priority): how many were dispelled, the median time to dispel, and the ones that weren't, with who had a dispel ready and in range</div><table><tr><th>Pull</th><th>Boss</th><th>Dispelled</th><th>Median</th><th>Not dispelled</th></tr>");
    for r in rows {
        o.push_str(&r);
    }
    o.push_str("</table>");
}

/// M+ damage your non-tanks took against the top runs (with `--compare-top`):
/// the spells you took most, with the ones the advice lists marked.
fn damage_html(f: &FightReport, o: &mut String) {
    let Some(r) = &f.reference else { return };
    let rows = crate::rules::damage::compare(f, r);
    if rows.is_empty() {
        return;
    }
    let _ = write!(
        o,
        "<h3>Damage taken vs top runs</h3><div class=sub>Non-tank damage per player per 10 minutes of combat. Top = median (range) of {} top +{} run(s); red = far more than they take.</div><table><tr><th>Spell</th><th>Yours</th><th>Top</th><th>×</th><th>Non-tank hits</th></tr>",
        r.damage_runs, r.level
    );
    // The 12 you took most, plus any the advice lists.
    for s in rows
        .iter()
        .enumerate()
        .filter(|(i, s)| *i < 12 || s.listed)
        .map(|(_, s)| s)
    {
        let ratio = if s.top.median > 0.0 {
            format!("{:.1}", s.mine / s.top.median)
        } else {
            "–".into()
        };
        let _ = write!(
            o,
            "<tr><td>{}</td><td class=n><span{}>{}</span></td><td class=n>{} <span class=sub>({}–{})</span></td><td class=n>{ratio}</td><td class=n>{}</td></tr>",
            esc(s.name),
            if s.listed { " class=bad" } else { "" },
            human(s.mine as u64),
            human(s.top.median as u64),
            human(s.top.min as u64),
            human(s.top.max as u64),
            s.hits
        );
    }
    o.push_str("</table>");
}

/// M+ route: one row per segment (boss to boss), against the top runs when
/// `--compare-top` was used, then the pulls with their kills and forces pace.
fn route_html(f: &FightReport, o: &mut String) {
    if f.segments.is_empty() {
        return;
    }
    let r = f.reference.as_ref();
    let rng = |s: &crate::reference::Stat| {
        format!(
            "{} <span class=sub>({}–{})</span>",
            mmss(s.median),
            mmss(s.min),
            mmss(s.max)
        )
    };
    let _ = write!(
        o,
        "<h3>Route by segment</h3><div class=sub>Each segment runs from one boss kill to the next. {}</div><table><tr><th>Segment</th><th>Time</th>{}<th>Forces at end</th><th>Combat</th><th>Downtime</th><th>Deaths</th></tr>",
        match r {
            Some(t) => format!(
                "Top = median (range) of {} top +{} public run(s) on Warcraft Logs.",
                t.runs.len(),
                t.level
            ),
            None => "Add --compare-top 5 to compare with top public runs.".into(),
        },
        if r.is_some() {
            "<th>Top</th><th>Δ</th>"
        } else {
            ""
        }
    );
    let mut prev: Option<&str> = None;
    for s in &f.segments {
        let label = match (prev, s.boss.as_deref()) {
            (None, Some(b)) => format!("start → {}", esc(b)),
            (Some(a), Some(b)) => format!("{} → {}", esc(a), esc(b)),
            (Some(a), None) => format!("after {}", esc(a)),
            (None, None) => "the key".into(),
        };
        prev = s.boss.as_deref();
        let t = r.and_then(|t| {
            t.segments
                .iter()
                .find(|x| x.index == s.index && x.boss == s.boss)
        });
        let mine = s.end_s - s.start_s;
        let top_cols = match (r, t) {
            (Some(_), Some(t)) => {
                let d = mine - t.duration_s.median;
                format!(
                    "<td class=n>{}</td><td class=n><span{}>{}{}</span></td>",
                    rng(&t.duration_s),
                    if d > 60.0 { " class=bad" } else { "" },
                    if d >= 0.0 { "+" } else { "−" },
                    mmss(d.abs())
                )
            }
            (Some(_), None) => "<td></td><td></td>".into(),
            _ => String::new(),
        };
        let boss_forces = r.and_then(|t| {
            let enc = match &f.kind {
                FightKind::MythicPlus { bosses, .. } => bosses
                    .iter()
                    .find(|b| b.success && Some(&b.name) == s.boss.as_ref())
                    .map(|b| b.encounter_id),
                _ => None,
            }?;
            t.bosses.iter().find(|b| b.encounter_id == enc)?.forces_pct
        });
        let forces = match (s.forces_pct, boss_forces) {
            (Some(m), Some(top)) => format!(
                "<span{}>{m:.0}%</span> <span class=sub>(top {:.0}%)</span>",
                if top.median - m > 5.0 {
                    " class=bad"
                } else {
                    ""
                },
                top.median
            ),
            (Some(m), None) => format!("{m:.0}%"),
            _ => String::new(),
        };
        let vs = |mine: f64, top: Option<f64>| match top {
            Some(t) => format!("{} <span class=sub>(top {})</span>", mmss(mine), mmss(t)),
            None => mmss(mine),
        };
        let _ = write!(
            o,
            "<tr><td>{label}</td><td class=n>{}</td>{top_cols}<td class=n>{forces}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
            mmss(mine),
            vs(s.combat_s, t.map(|t| t.combat_s.median)),
            vs(s.downtime_s, t.map(|t| t.downtime_s.median)),
            if s.deaths > 0 {
                format!(
                    "{} <span class=sub>({})</span>",
                    s.deaths,
                    mmss(s.death_penalty_s)
                )
            } else {
                String::new()
            }
        );
    }
    o.push_str("</table>");

    o.push_str("<h3>Pulls</h3><table><tr><th>Pull</th><th>Start</th><th>Length</th><th>Killed</th><th>Forces</th><th>%/min</th></tr>");
    for p in &f.pulls {
        let len = p.end_s - p.start_s;
        let killed = match &p.boss {
            Some(b) => format!("<b>{}</b>", esc(b)),
            None => p
                .npcs
                .iter()
                .take(4)
                .map(|n| format!("{} ×{}", esc(&n.name), n.count))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let _ = write!(
            o,
            "<tr><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td>{killed}</td><td class=n>{}</td><td class=n>{}</td></tr>",
            p.index,
            mmss(p.start_s),
            mmss(len),
            p.forces_pct.map(|x| format!("{x:.1}%")).unwrap_or_default(),
            match p.forces_pct {
                Some(x) if p.boss.is_none() && len > 0.0 => format!("{:.1}", x * 60.0 / len),
                _ => String::new(),
            }
        );
    }
    o.push_str("</table>");
}

pub fn html(r: &Report) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<!DOCTYPE html><html><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>wowcoach report</title><style>{CSS}</style></head><body><main>"
    );
    let _ = write!(
        o,
        "<h1>wowcoach report</h1><div class=sub>{} · {} fights analysed{}</div>",
        esc(&r.source),
        r.fights.len(),
        r.season_pack
            .as_deref()
            .map(|k| format!(" · season pack {}", esc(k)))
            .unwrap_or_default()
    );

    for p in &r.progression {
        let _ = write!(
            o,
            "<div class=card><details open><summary><h2>Progression: {} {}</h2><span class=chip>{} pulls</span><span class='chip {}'>{} kills</span>{}</summary><div class=body>",
            esc(&p.difficulty),
            esc(&p.name),
            p.attempts,
            if p.kills > 0 { "ok" } else { "bad" },
            p.kills,
            p.best_hp_pct
                .map(|b| format!("<span class=chip>best {b:.1}%</span>"))
                .unwrap_or_default()
        );
        suggestions_html(&p.suggestions, &mut o);
        o.push_str("<div class=grid>");
        for (title, rows) in [
            ("Killing blows", &p.death_causes),
            ("First death", &p.first_death_causes),
            ("Deaths by player", &p.frequent_deaths),
        ] {
            let _ = write!(o, "<div><h3>{title}</h3><table>");
            for (k, n) in rows.iter() {
                let _ = write!(o, "<tr><td>{}</td><td class=n>{n}</td></tr>", esc(k));
            }
            o.push_str("</table></div>");
        }
        o.push_str("</div></div></details></div>");
    }

    for k in &r.kick_plans {
        kick_plan_html(k, &mut o);
    }

    for f in &r.fights {
        let ok = match &f.kind {
            FightKind::Raid { success, .. } => *success,
            // Timed (beat the pack's timer); a completed key with no known
            // timer counts as ok.
            FightKind::MythicPlus {
                success, upgrades, ..
            } => upgrades.map_or(*success, |u| u > 0),
        };
        let high = f
            .suggestions
            .iter()
            .filter(|s| s.severity == Severity::High)
            .count();
        let _ = write!(
            o,
            "<div class=card><details{}><summary><h2>{}</h2><span class='chip {}'>{}</span><span class=chip>{} deaths</span><span class=chip>{} suggestions ({} high)</span>{}</summary><div class=body>",
            if high > 0 || r.fights.len() <= 3 {
                " open"
            } else {
                ""
            },
            esc(&f.title),
            if ok { "ok" } else { "bad" },
            mmss(f.duration_s),
            f.deaths.len(),
            f.suggestions.len(),
            high,
            if f.complete {
                ""
            } else {
                "<span class=chip>incomplete</span>"
            }
        );

        if let FightKind::MythicPlus { bosses, .. } = &f.kind {
            o.push_str("<h3>Run timeline</h3><div class=timeline>");
            for p in &f.pulls {
                let _ = write!(
                    o,
                    "<i class='{}' title='Pull {} {}–{} · {} deaths{}' style='left:{:.2}%;width:{:.2}%'></i>",
                    if p.deaths > 0 { "d" } else { "" },
                    p.index,
                    mmss(p.start_s),
                    mmss(p.end_s),
                    p.deaths,
                    p.boss
                        .as_ref()
                        .map(|b| format!(" · {}", esc(b)))
                        .unwrap_or_default(),
                    100.0 * p.start_s / f.duration_s,
                    (100.0 * (p.end_s - p.start_s) / f.duration_s).max(0.3)
                );
            }
            for b in bosses {
                let _ = write!(
                    o,
                    "<b title='{}{}' style='left:{:.2}%'></b>",
                    esc(&b.name),
                    b.forces_pct
                        .map(|p| format!(" · {p:.0}% forces at the kill"))
                        .unwrap_or_default(),
                    100.0 * b.start_s / f.duration_s
                );
            }
            let _ = write!(
                o,
                "</div><div class=sub>{} pulls · {} out of combat ({:.0}%) · blue = in combat, red = pull with a death, yellow = boss</div>",
                f.pulls.len(),
                mmss(f.downtime_s),
                100.0 * f.downtime_s / f.duration_s.max(1.0)
            );
        }

        suggestions_html(&f.suggestions, &mut o);
        if matches!(f.kind, FightKind::MythicPlus { .. }) {
            priority_casts_html(f, &mut o);
            defensives_html(f, &mut o);
            dispels_html(f, &mut o);
            route_html(f, &mut o);
            damage_html(f, &mut o);
        }

        // players
        let max_dps = f.players.iter().map(|p| p.dps).fold(0.0, f64::max).max(1.0);
        let is_mplus = matches!(f.kind, FightKind::MythicPlus { .. });
        let has_rank = f.players.iter().any(|p| p.wcl_rank_percent.is_some());
        let _ = write!(
            o,
            "<h3>Players</h3><table><tr><th>Player</th><th>Spec</th><th>DPS</th><th></th><th>HPS</th><th>Shields</th><th>Taken</th><th>Avoidable</th><th>Kicks</th>{}<th>Dispels</th><th>Deaths</th><th>Activity</th>{}</tr>",
            if is_mplus {
                "<th title='median time into the cast bar'>Kick time</th><th title='priority casts that went off with your kick ready in range'>Missed ★</th><th title='filler kicked while a priority cast was casting'>Wasted</th><th title='median time to dispel a priority debuff'>Dispel time</th>"
            } else {
                ""
            },
            if has_rank { "<th>WCL %</th>" } else { "" }
        );
        for p in &f.players {
            let _ = write!(
                o,
                "<tr><td>{}</td><td>{} {}{}</td><td class=n>{}</td><td><div class=bar><i style='width:{:.0}%'></i></div></td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td>{}<td class=n>{}</td><td class=n>{}</td><td class=n>{:.0}%</td>{}</tr>",
                esc(&p.name),
                esc(&p.spec),
                esc(&p.class),
                if p.role_inferred {
                    format!(" <span class=sub>({:?}?)</span>", p.role)
                } else {
                    String::new()
                },
                human(p.dps as u64),
                100.0 * p.dps / max_dps,
                human(p.hps as u64),
                if p.absorbs_done > 0 {
                    human(p.absorbs_done)
                } else {
                    String::new()
                },
                human(p.damage_taken),
                human(p.avoidable_taken + p.likely_avoidable_taken),
                p.interrupts,
                if is_mplus {
                    format!(
                        "<td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td>",
                        p.kick_latency_s
                            .map(|l| format!("{l:.1}s"))
                            .unwrap_or_default(),
                        if p.priority_missed > 0 {
                            p.priority_missed.to_string()
                        } else {
                            String::new()
                        },
                        if p.wasted_kicks > 0 {
                            p.wasted_kicks.to_string()
                        } else {
                            String::new()
                        },
                        p.dispel_latency_s
                            .map(|l| format!(
                                "<span title='{} priority dispel(s)'>{l:.1}s</span>",
                                p.priority_dispels
                            ))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                },
                p.dispels,
                p.deaths,
                p.activity_pct * 100.0,
                if has_rank {
                    format!(
                        "<td class=n>{}</td>",
                        p.wcl_rank_percent
                            .map(|r| format!("{r:.0}"))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                }
            );
        }
        o.push_str("</table>");

        if !f.deaths.is_empty() {
            o.push_str("<h3>Deaths</h3><table><tr><th>Time</th><th>Player</th><th>Killing blow</th><th>Last hits</th></tr>");
            for d in &f.deaths {
                let recap = d
                    .recap
                    .iter()
                    .map(|r| {
                        format!(
                            "{} {} {}{}",
                            mmss(r.at_s),
                            esc(&r.spell),
                            human(r.amount),
                            r.hp_pct_after
                                .map(|h| format!(" ({h:.0}%)"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("<br>");
                let _ = write!(
                    o,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td class=sub>{}</td></tr>",
                    mmss(d.at_s),
                    esc(&d.player),
                    if d.priority_debuffs.is_empty() {
                        esc(&d.killing_blow)
                    } else {
                        format!(
                            "{} <span class=bad>· {} up</span>",
                            esc(&d.killing_blow),
                            esc(&d.priority_debuffs.join(", "))
                        )
                    },
                    recap
                );
            }
            o.push_str("</table>");
        }

        o.push_str("<div class=grid>");
        if !f.enemy_casts.is_empty() {
            o.push_str("<div><h3>Interruptible casts</h3><table><tr><th>Spell</th><th>Went off</th><th>Kicked</th></tr>");
            for c in &f.enemy_casts {
                let _ = write!(
                    o,
                    "<tr><td>{}{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
                    esc(&c.name),
                    if c.priority {
                        " ★"
                    } else if c.kick {
                        " ☆"
                    } else {
                        ""
                    },
                    c.succeeded,
                    c.interrupted
                );
            }
            o.push_str("</table></div>");
        }
        o.push_str("<div><h3>Damage taken by enemy spell</h3><table><tr><th>Spell</th><th>Total</th><th>Players hit</th><th></th></tr>");
        for s in f.enemy_damage.iter().take(12) {
            let _ = write!(
                o,
                "<tr><td>{}</td><td class=n>{}</td><td class=n>{}</td><td>{}</td></tr>",
                esc(&s.name),
                human(s.total),
                s.players_hit,
                if s.configured_avoidable {
                    "<span class='chip bad'>avoidable</span>"
                } else if s.likely_avoidable {
                    "<span class=chip>check</span>"
                } else {
                    ""
                }
            );
        }
        o.push_str("</table></div></div></div></details></div>");
    }
    o.push_str("<p class=sub>wowcoach · Activity and the \"possibly avoidable\" flags are heuristics. Confirm them against the fight before you act on them.</p></main></body></html>");
    o
}
