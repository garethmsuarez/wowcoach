use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[cfg(feature = "wcl")]
use wowcoach_core::analysis::Analyzer;
use wowcoach_core::analysis::{self, FightKind, Severity};
use wowcoach_core::config::{self, Config, PackChoice};
use wowcoach_core::report::{self, Report};
use wowcoach_core::season::SeasonPack;
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
    /// Config with thresholds and overrides of the season pack
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Use this season pack instead of the built-in one (e.g. while editing it)
    #[arg(long, value_name = "PATH", conflicts_with = "no_season_pack")]
    season_pack: Option<PathBuf>,
    /// Don't use any season pack: only the spells in --config
    #[arg(long)]
    no_season_pack: bool,
}

impl OutOpts {
    fn load_config(&self) -> anyhow::Result<Config> {
        let pack = match (&self.season_pack, self.no_season_pack) {
            (Some(p), _) => PackChoice::File(p.clone()),
            (None, true) => PackChoice::None,
            (None, false) => PackChoice::Embedded,
        };
        Config::load_with(self.config.as_deref(), pack)
    }
}

#[derive(Subcommand)]
enum SeasonCmd {
    /// Validate a season pack and summarise it (default: the built-in pack)
    Check {
        /// Pack file, e.g. data/seasons/midnight-s2.toml
        path: Option<PathBuf>,
    },
    /// Update a pack from the Blizzard API: current dungeon pool, timers,
    /// bosses and journal spells. Tags, notes and logged IDs are kept.
    #[cfg(feature = "blizzard")]
    Sync {
        /// Pack file to update in place
        #[arg(long, default_value = "data/seasons/midnight-s2.toml")]
        pack: PathBuf,
        /// Raid journal instance IDs to include (default: the raids already in the pack)
        #[arg(long = "raid", value_name = "JOURNAL_ID")]
        raids: Vec<u32>,
        /// API region: us, eu, kr or tw
        #[arg(long, default_value = "us")]
        region: String,
        /// Show what would change without writing the pack
        #[arg(long)]
        dry_run: bool,
        /// Battle.net client ID (or BLIZZARD_CLIENT_ID, also read from ./.env)
        #[arg(long, env = "BLIZZARD_CLIENT_ID", hide_env_values = true)]
        client_id: String,
        /// Battle.net client secret (or BLIZZARD_CLIENT_SECRET, also read from ./.env)
        #[arg(long, env = "BLIZZARD_CLIENT_SECRET", hide_env_values = true)]
        client_secret: String,
    },
}

#[cfg(feature = "blizzard")]
fn season_sync(
    pack_path: &std::path::Path,
    raids: Vec<u32>,
    region: &str,
    dry_run: bool,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<()> {
    use wowcoach_core::blizzard;
    let previous = std::fs::read_to_string(pack_path)
        .with_context(|| format!("reading {}", pack_path.display()))?;
    let mut pack = SeasonPack::parse(&previous)
        .with_context(|| format!("{} is not a valid pack", pack_path.display()))?;
    let raids = if raids.is_empty() {
        pack.raids
            .iter()
            .map(|r| r.journal_id)
            .filter(|id| *id != 0)
            .collect()
    } else {
        raids
    };

    eprintln!("Connecting to Battle.net ({region})...");
    let client = blizzard::net::Client::connect(client_id, client_secret, region)?;
    eprintln!("Scanning keystone dungeons for the current pool...");
    let synced = blizzard::fetch(&client, &raids, |line| eprintln!("  {line}"))?;
    let wcl = wcl_encounters();

    let summary = blizzard::merge(&mut pack, &synced, &wcl);
    let problems = pack.problems();
    if !problems.is_empty() {
        anyhow::bail!(
            "the merged pack is invalid, nothing written:\n  - {}",
            problems.join("\n  - ")
        );
    }
    let text = blizzard::render(&pack, &previous)?;
    print!("{summary}");
    println!("{}", pack.summary());
    if dry_run {
        println!("Dry run: {} not written.", pack_path.display());
    } else if text == previous {
        println!("{} is already up to date.", pack_path.display());
    } else {
        std::fs::write(pack_path, text)
            .with_context(|| format!("writing {}", pack_path.display()))?;
        println!(
            "Wrote {}. Review the diff before committing.",
            pack_path.display()
        );
    }
    Ok(())
}

/// Raid boss name -> logged encounter ID from Warcraft Logs, when its
/// credentials are available. Optional: sync works without it.
#[cfg(feature = "blizzard")]
fn wcl_encounters() -> Vec<(String, u32)> {
    #[cfg(feature = "wcl")]
    if let (Ok(id), Ok(secret)) = (
        std::env::var("WCL_CLIENT_ID"),
        std::env::var("WCL_CLIENT_SECRET"),
    ) {
        eprintln!("Looking up encounter IDs on Warcraft Logs...");
        match wcl::net::Client::connect(&id, &secret).and_then(|c| c.zone_encounters()) {
            Ok(v) => return v,
            Err(e) => eprintln!("  skipped: {e:#}"),
        }
        return vec![];
    }
    eprintln!(
        "No WCL_CLIENT_ID/WCL_CLIENT_SECRET: raid encounter IDs are left for `season suggest`."
    );
    vec![]
}

/// Read API credentials from ./.env without overriding the environment.
fn load_dotenv() {
    for (k, v) in wowcoach_core::dotenv::missing_from_env(std::path::Path::new(".env")) {
        // SAFETY: runs first thing in main, before any other thread exists.
        unsafe { std::env::set_var(k, v) };
    }
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
    /// Season data pack: dungeons, bosses and tagged enemy spells
    Season {
        #[command(subcommand)]
        cmd: SeasonCmd,
    },
    /// Write the default config (thresholds and season pack overrides)
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
    load_dotenv();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::InitConfig { out } => {
            std::fs::write(&out, config::DEFAULT_CONFIG)?;
            println!("Wrote {}", out.display());
        }
        Cmd::Season {
            cmd: SeasonCmd::Check { path },
        } => {
            let pack = match &path {
                Some(p) => SeasonPack::load(p)?,
                None => SeasonPack::embedded(),
            };
            println!("{}", pack.summary());
        }
        #[cfg(feature = "blizzard")]
        Cmd::Season {
            cmd:
                SeasonCmd::Sync {
                    pack,
                    raids,
                    region,
                    dry_run,
                    client_id,
                    client_secret,
                },
        } => season_sync(&pack, raids, &region, dry_run, &client_id, &client_secret)?,
        Cmd::Analyze { log, o } => {
            let cfg = o.load_config()?;
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
            let cfg = o.load_config()?;
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
