use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[cfg(feature = "wcl")]
use wowcoach_core::analysis::Analyzer;
use wowcoach_core::analysis::{self, FightKind, Severity};
use wowcoach_core::config::{self, Config};
use wowcoach_core::report::{self, Report};
#[cfg(feature = "wcl")]
use wowcoach_core::wcl;

#[derive(Parser)]
#[command(
    name = "wowcoach",
    version,
    about = "Turn WoW combat logs into improvement suggestions for Mythic+ and raid"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args)]
struct OutOpts {
    /// HTML report path
    #[arg(short, long, default_value = "wowcoach-report.html")]
    out: PathBuf,
    /// Also write machine-readable JSON here
    #[arg(long)]
    json: Option<PathBuf>,
    /// Config with thresholds and avoidable/priority-kick spell lists
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Analyze a raw WoWCombatLog*.txt file
    Analyze {
        log: PathBuf,
        #[command(flatten)]
        o: OutOpts,
    },
    /// Fetch and analyze a Warcraft Logs report (public reports)
    #[cfg(feature = "wcl")]
    Wcl {
        /// Report code, e.g. the `aBcD1234` in warcraftlogs.com/reports/aBcD1234
        code: String,
        /// Only these fight IDs (default: all boss and keystone fights)
        #[arg(long = "fight")]
        fights: Vec<i64>,
        #[arg(long, env = "WCL_CLIENT_ID")]
        client_id: String,
        #[arg(long, env = "WCL_CLIENT_SECRET", hide_env_values = true)]
        client_secret: String,
        /// Skip the per-fight rankings lookup
        #[arg(long)]
        no_rankings: bool,
        #[command(flatten)]
        o: OutOpts,
    },
    /// Write the default config so you can add spell IDs for this season
    InitConfig {
        #[arg(short, long, default_value = "wowcoach.toml")]
        out: PathBuf,
    },
}

fn finish(
    source: String,
    fights: Vec<analysis::FightReport>,
    cfg: &Config,
    o: &OutOpts,
) -> anyhow::Result<()> {
    let rep = wowcoach_core::build_report(source, fights, cfg);
    std::fs::write(&o.out, report::html(&rep))
        .with_context(|| format!("writing {}", o.out.display()))?;
    if let Some(j) = &o.json {
        std::fs::write(j, serde_json::to_string_pretty(&rep)?)?;
    }
    print_summary(&rep);
    eprintln!("\nReport written to {}", o.out.display());
    Ok(())
}

fn print_summary(r: &Report) {
    if r.fights.is_empty() {
        println!(
            "No raid encounters or Mythic+ runs were found. Was advanced combat logging on, and did the log cover a boss pull or a key?"
        );
    }
    for f in &r.fights {
        let tag = match f.kind {
            FightKind::Raid { group_size, .. } if group_size > 5 => "RAID",
            FightKind::Raid { .. } => "BOSS",
            FightKind::MythicPlus { .. } => "M+  ",
        };
        let high: Vec<_> = f
            .suggestions
            .iter()
            .filter(|s| s.severity == Severity::High)
            .collect();
        println!(
            "[{tag}] {}  ({:.0}s, {} deaths, {} suggestions)",
            f.title,
            f.duration_s,
            f.deaths.len(),
            f.suggestions.len()
        );
        for s in high.iter().take(4) {
            println!("   ! [{}] {}: {}", s.scope, s.category, s.message);
        }
    }
    for p in &r.progression {
        println!(
            "[PROG] {} {}: {} pulls, {} kills",
            p.difficulty, p.name, p.attempts, p.kills
        );
        for s in p.suggestions.iter().take(3) {
            println!("   ! [{}] {}", s.scope, s.message);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::InitConfig { out } => {
            std::fs::write(&out, config::DEFAULT_CONFIG)?;
            println!("Wrote {}", out.display());
        }
        Cmd::Analyze { log, o } => {
            let cfg = Config::load(o.config.as_deref())?;
            let file =
                std::fs::File::open(&log).with_context(|| format!("opening {}", log.display()))?;
            let t0 = std::time::Instant::now();
            let (p, fights, cfg) =
                wowcoach_core::analyze_log(std::io::BufReader::with_capacity(1 << 20, file), cfg)?;
            eprintln!(
                "Parsed {} lines in {:.1}s (log version {}, advanced logging {}, {} advanced fields)",
                p.lines,
                t0.elapsed().as_secs_f64(),
                p.log_version.map(|v| v.to_string()).unwrap_or("?".into()),
                if p.advanced {
                    "on"
                } else {
                    "OFF: enable it in System > Network for HP data"
                },
                p.adv_fields
            );
            finish(log.display().to_string(), fights, &cfg, &o)?;
        }
        #[cfg(feature = "wcl")]
        Cmd::Wcl {
            code,
            fights,
            client_id,
            client_secret,
            no_rankings,
            o,
        } => {
            let cfg = Config::load(o.config.as_deref())?;
            eprintln!("Connecting to Warcraft Logs...");
            let client = wcl::net::Client::connect(&client_id, &client_secret)?;
            let fetched = wcl::net::fetch(&client, &code, &fights, !no_rankings)?;
            let mut an = Analyzer::new(cfg);
            for e in fetched.events {
                an.feed(e);
            }
            let (mut reports, cfg) = an.finish();
            for f in reports.iter_mut() {
                if let Some(r) = fetched.rankings.get(&f.start_ts) {
                    for p in f.players.iter_mut() {
                        p.wcl_rank_percent = r.get(&p.name).copied();
                    }
                }
                if let (
                    Some(pct),
                    FightKind::Raid {
                        boss_hp_pct,
                        success,
                        name,
                        difficulty,
                        ..
                    },
                ) = (fetched.boss_pct.get(&f.start_ts), &mut f.kind)
                {
                    *boss_hp_pct = Some(*pct);
                    if !*success {
                        f.title = format!("{difficulty} {name} — Wipe ({pct:.1}%)");
                    }
                }
            }
            finish(
                format!("Warcraft Logs: {} ({code})", fetched.title),
                reports,
                &cfg,
                &o,
            )?;
        }
    }
    Ok(())
}
