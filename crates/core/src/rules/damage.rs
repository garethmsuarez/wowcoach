//! "Damage top runs don't take" (M+, with `analyze --compare-top`): enemy
//! spells your non-tank players took far more of than the top public runs of
//! the same dungeon and level, per player and per 10 minutes of combat. It
//! needs no tags: a spell that hits everyone takes the same toll in top runs
//! and is never listed, while a dodgeable or kickable one stands out. Reads
//! `FightReport.enemy_damage` and `FightReport.reference.damage`.

use super::{LIST_MAX, human, sugg};
use crate::analysis::*;
use crate::config::Config;
use crate::reference::{RATE_MS, Stat, TopRuns};
use crate::season::Tag;
use crate::specs::Role;

/// A spell is listed when you take at least this many times the top median.
pub const VS_TOP_RATIO: f64 = 2.0;
/// ...your non-tanks were hit by it at least this often...
pub const VS_TOP_MIN_HITS: u32 = 5;
/// ...and the extra is at least this share of your non-tank damage taken.
pub const VS_TOP_MIN_SHARE: f64 = 0.02;
/// Medium severity from this share (the worst listed spell).
const VS_TOP_MEDIUM_SHARE: f64 = 0.05;
/// The log's spell ID for melee swings.
const MELEE: u32 = 1;
/// Fewer runs with damage data than this compare nothing.
const VS_TOP_MIN_RUNS: usize = 3;

/// One spell (all its IDs) as your non-tank players took it, against the top
/// runs. Rates are per non-tank player per 10 minutes of combat.
pub struct SpellVsTop<'a> {
    pub name: &'a str,
    pub ids: Vec<u32>,
    pub mine: f64,
    pub top: Stat,
    pub hits: u32,
    /// The extra over the top median, as a share of your non-tank damage.
    pub share: f64,
    /// Meets the bar for the "Damage vs top runs" advice.
    pub listed: bool,
    /// (player, hits), most first.
    pub players: Vec<(&'a str, u32)>,
}

/// Every spell your non-tanks took that the top runs' damage data covers,
/// most damage first. Empty with fewer than `VS_TOP_MIN_RUNS` damage runs.
pub fn compare<'a>(f: &'a FightReport, r: &TopRuns) -> Vec<SpellVsTop<'a>> {
    if r.damage_runs < VS_TOP_MIN_RUNS {
        return vec![];
    }
    let non_tanks: Vec<&str> = f
        .players
        .iter()
        .filter(|p| p.role != Role::Tank)
        .map(|p| p.name.as_str())
        .collect();
    let combat_ms: f64 = f.segments.iter().map(|s| s.combat_s).sum::<f64>() * 1000.0;
    if non_tanks.is_empty() || combat_ms <= 0.0 {
        return vec![];
    }
    let per = non_tanks.len() as f64 * combat_ms / RATE_MS;

    let mut own: Vec<(SpellVsTop, u64)> = vec![];
    for s in &f.enemy_damage {
        let i = match own.iter().position(|o| o.0.name == s.name) {
            Some(i) => i,
            None => {
                own.push((
                    SpellVsTop {
                        name: &s.name,
                        ids: vec![],
                        mine: 0.0,
                        top: Stat::default(),
                        hits: 0,
                        share: 0.0,
                        listed: false,
                        players: vec![],
                    },
                    0,
                ));
                own.len() - 1
            }
        };
        let (o, damage) = &mut own[i];
        o.ids.push(s.spell_id);
        for (name, dmg, hits) in s
            .per_player
            .iter()
            .filter(|p| non_tanks.contains(&p.0.as_str()))
        {
            *damage += dmg;
            o.hits += hits;
            match o.players.iter_mut().find(|p| p.0 == name) {
                Some(p) => p.1 += hits,
                None => o.players.push((name, *hits)),
            }
        }
    }
    let total: f64 = own.iter().map(|o| o.1 as f64).sum::<f64>() / per;
    let mut out: Vec<SpellVsTop> = own
        .into_iter()
        .filter(|(_, damage)| *damage > 0)
        .filter_map(|(mut o, damage)| {
            o.top = r.damage.iter().find(|t| t.name == o.name)?.damage;
            o.mine = damage as f64 / per;
            o.share = (o.mine - o.top.median) / total;
            o.listed = o.hits >= VS_TOP_MIN_HITS
                && o.mine >= VS_TOP_RATIO * o.top.median
                && o.share >= VS_TOP_MIN_SHARE;
            o.players
                .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            Some(o)
        })
        .collect();
    out.sort_by(|a, b| b.mine.total_cmp(&a.mine).then_with(|| a.name.cmp(b.name)));
    out
}

pub(super) fn vs_top(f: &FightReport, r: &TopRuns, cfg: &Config, out: &mut Vec<Suggestion>) {
    let mut listed: Vec<SpellVsTop> = compare(f, r).into_iter().filter(|s| s.listed).collect();
    if listed.is_empty() {
        return;
    }
    listed.sort_by(|a, b| b.share.total_cmp(&a.share).then_with(|| a.name.cmp(b.name)));

    let list = listed
        .iter()
        .take(LIST_MAX)
        .map(|o| {
            let (mine, top) = (o.mine, o.top.median);
            let ratio = if top > 0.0 {
                format!("{:.1}×", mine / top)
            } else {
                "top runs took none".into()
            };
            let tag = if o.ids.iter().any(|id| cfg.kick_tier(*id) == Some(1)) {
                ", a must-kick"
            } else if o.ids.iter().any(|id| cfg.kick_tier(*id) == Some(2)) {
                ", kick when free"
            } else if o.ids.iter().any(|id| cfg.has_tag(*id, Tag::Avoidable)) {
                ", avoidable"
            } else if o.ids.contains(&MELEE) {
                ", enemies loose on non-tanks"
            } else {
                ""
            };
            let most = o
                .players
                .iter()
                .take(2)
                .map(|(p, h)| format!("{p} ×{h}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{} {} vs {} ({ratio}{tag}; most hits: {most})",
                o.name,
                human(mine as u64),
                human(top as u64)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    out.push(sugg(
        "group",
        if listed[0].share >= VS_TOP_MEDIUM_SHARE { Severity::Medium } else { Severity::Low },
        "Damage vs top runs",
        format!(
            "Your group took far more of these than the top +{} runs, per non-tank player per 10 minutes of combat: {list}{}.",
            r.level,
            if listed.len() > LIST_MAX { "; …" } else { "" }
        ),
        format!(
            "Compared with the median of {} top run(s) with damage data. Listed at {VS_TOP_RATIO}× their damage or more, at least {VS_TOP_MIN_HITS} hits on your non-tanks, and an extra of at least {:.0}% of your non-tank damage taken. A spell that hits everyone costs top runs the same and isn't listed.",
            r.damage_runs,
            VS_TOP_MIN_SHARE * 100.0
        ),
    ));
}
