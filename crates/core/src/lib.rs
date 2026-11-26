//! wowcoach core: combat log and Warcraft Logs adapters, fight analysis,
//! coaching rules and report rendering. Front ends (the CLI, later a server)
//! only handle I/O around [`build_report`].

pub mod analysis;
pub mod combatlog;
pub mod config;
pub mod model;
pub mod report;
pub mod rules;
pub mod season;
pub mod specs;
#[cfg(test)]
mod testutil;
#[cfg(feature = "wcl")]
pub mod wcl;

use analysis::{Analyzer, FightReport};
use config::Config;
use report::Report;

/// Apply the rules to analysed fights and assemble the report, including the
/// cross-pull progression summary.
pub fn build_report(source: String, mut fights: Vec<FightReport>, cfg: &Config) -> Report {
    for f in fights.iter_mut() {
        rules::apply(f, cfg);
    }
    let mut progression = analysis::progression(&fights);
    for p in progression.iter_mut() {
        rules::apply_progression(p);
    }
    Report {
        source,
        season_pack: cfg.knowledge.pack_key.clone(),
        fights,
        progression,
    }
}

/// Parse a raw combat log and analyse it. Returns the parser (for log
/// version/field-count diagnostics), the fights and the config back.
pub fn analyze_log<R: std::io::BufRead>(
    reader: R,
    cfg: Config,
) -> std::io::Result<(combatlog::Parser, Vec<FightReport>, Config)> {
    let mut an = Analyzer::new(cfg);
    let p = combatlog::stream(reader, |e| an.feed(e))?;
    let (fights, cfg) = an.finish();
    Ok((p, fights, cfg))
}
