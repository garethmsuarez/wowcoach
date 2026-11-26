//! Self-contained HTML report.

use crate::analysis::*;
use crate::rules::human;
use serde::Serialize;
use std::fmt::Write;

#[derive(Serialize)]
pub struct Report {
    pub source: String,
    pub fights: Vec<FightReport>,
    pub progression: Vec<ProgressionReport>,
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

pub fn html(r: &Report) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<!DOCTYPE html><html><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>wowcoach report</title><style>{CSS}</style></head><body><main>"
    );
    let _ = write!(
        o,
        "<h1>wowcoach report</h1><div class=sub>{} · {} fights analysed</div>",
        esc(&r.source),
        r.fights.len()
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

    for f in &r.fights {
        let ok = match &f.kind {
            FightKind::Raid { success, .. } | FightKind::MythicPlus { success, .. } => *success,
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
                    "<b title='{}' style='left:{:.2}%'></b>",
                    esc(&b.name),
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

        // players
        let max_dps = f.players.iter().map(|p| p.dps).fold(0.0, f64::max).max(1.0);
        let has_rank = f.players.iter().any(|p| p.wcl_rank_percent.is_some());
        let _ = write!(
            o,
            "<h3>Players</h3><table><tr><th>Player</th><th>Spec</th><th>DPS</th><th></th><th>HPS</th><th>Shields</th><th>Taken</th><th>Avoidable</th><th>Kicks</th><th>Dispels</th><th>Deaths</th><th>Activity</th>{}</tr>",
            if has_rank { "<th>WCL %</th>" } else { "" }
        );
        for p in &f.players {
            let _ = write!(
                o,
                "<tr><td>{}</td><td>{} {}{}</td><td class=n>{}</td><td><div class=bar><i style='width:{:.0}%'></i></div></td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{:.0}%</td>{}</tr>",
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
                    esc(&d.killing_blow),
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
                    if c.priority { " ★" } else { "" },
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
