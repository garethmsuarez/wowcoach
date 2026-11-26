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
    /// Learn spell and boss encounter IDs from logs and propose tags for review.
    /// Proposals go to `suggested`; tags are never set.
    Suggest(SuggestArgs),
}

#[derive(Subcommand)]
enum ClassesCmd {
    /// Validate the class table; with logs, check it against what players did
    Check {
        /// Raw combat logs to audit against
        logs: Vec<PathBuf>,
        /// Table file (default: the built-in data/classes.toml)
        #[arg(long)]
        table: Option<PathBuf>,
        /// Also check every spell ID and name against the Blizzard spell API
        #[cfg(feature = "blizzard")]
        #[arg(long)]
        live: bool,
        /// API region for --live
        #[cfg(feature = "blizzard")]
        #[arg(long, default_value = "us")]
        region: String,
    },
}

fn classes_check(
    table: Option<PathBuf>,
    logs: &[PathBuf],
    live: Option<&str>,
) -> anyhow::Result<()> {
    use wowcoach_core::classes::{Audit, ClassData};
    let data = match &table {
        Some(p) => ClassData::load(p)?,
        None => ClassData::embedded(),
    };
    println!("{}", data.summary());
    let mut failed = false;

    #[cfg(feature = "blizzard")]
    if let Some(region) = live {
        let (Ok(id), Ok(secret)) = (
            std::env::var("BLIZZARD_CLIENT_ID"),
            std::env::var("BLIZZARD_CLIENT_SECRET"),
        ) else {
            anyhow::bail!(
                "--live needs BLIZZARD_CLIENT_ID and BLIZZARD_CLIENT_SECRET (env or ./.env)"
            );
        };
        let client = wowcoach_core::blizzard::net::Client::connect(&id, &secret, region)?;
        let spells: Vec<(u32, String)> = data
            .abilities
            .iter()
            .flat_map(|a| a.ids.iter().map(|id| (*id, a.name.clone())))
            .collect();
        eprintln!(
            "Checking {} spell IDs against the Blizzard API...",
            spells.len()
        );
        let (unknown, renamed) = wowcoach_core::blizzard::check_spells(&client, &spells)?;
        if !unknown.is_empty() {
            println!(
                "Not in the spell API ({}): effect-only IDs can be absent, anything else is a wrong ID:",
                unknown.len()
            );
            for u in &unknown {
                println!("  {u}");
            }
        }
        if !renamed.is_empty() {
            println!("Name differs from the API ({}):", renamed.len());
            for r in &renamed {
                println!("  {r}");
            }
        }
        if unknown.is_empty() && renamed.is_empty() {
            println!("All {} spell IDs exist with matching names.", spells.len());
        }
    }
    #[cfg(not(feature = "blizzard"))]
    let _ = live;

    if !logs.is_empty() {
        let mut audit = Audit::default();
        for log in logs {
            let file =
                std::fs::File::open(log).with_context(|| format!("opening {}", log.display()))?;
            wowcoach_core::combatlog::stream(
                std::io::BufReader::with_capacity(1 << 20, file),
                |e| audit.feed(&e),
            )?;
        }
        let r = audit.report(&data);
        print!("{r}");
        failed |= !r.missing_interrupts.is_empty();
    }
    if failed {
        anyhow::bail!("the class table is missing interrupts that players used");
    }
    Ok(())
}

#[derive(clap::Args)]
struct SuggestArgs {
    /// Your raw combat logs
    logs: Vec<PathBuf>,
    /// Pack file to update
    #[arg(long, default_value = "data/seasons/midnight-s2.toml")]
    pack: PathBuf,
    /// Write the pack (default: only print the review)
    #[arg(long)]
    write: bool,
    /// Your Warcraft Logs reports, as CODE or CODE:FIGHT
    #[cfg(feature = "wcl")]
    #[arg(long = "wcl", value_name = "CODE[:FIGHT]")]
    wcl: Vec<String>,
    /// Also analyse the top N public runs of each pack dungeon on Warcraft Logs
    #[cfg(feature = "wcl")]
    #[arg(long, default_value_t = 0, value_name = "N")]
    wcl_top: usize,
    /// Key levels for --wcl-top: MIN..MAX, a single level, or "top" (the
    /// overall best runs). Repeatable; ":N" takes N runs from that range
    /// instead of --wcl-top, e.g. --level 12..16 --level top:3
    #[cfg(feature = "wcl")]
    #[arg(long = "level", value_name = "RANGE[:N]")]
    levels: Vec<String>,
    /// Most Warcraft Logs API points to spend in this run (the hourly limit is 9000)
    #[cfg(feature = "wcl")]
    #[arg(long, default_value_t = 3000.0)]
    max_points: f64,
    /// Where analysed public runs are cached (default: ~/.cache/wowcoach/suggest)
    #[cfg(feature = "wcl")]
    #[arg(long)]
    cache_dir: Option<PathBuf>,
}

/// One --level: Some((min, max)) or None for "top", and how many runs to take.
#[cfg(feature = "wcl")]
#[derive(Debug, PartialEq)]
struct LevelSpec {
    range: Option<(u32, u32)>,
    count: usize,
}

/// "12..16" -> 12..=16, "20" -> 20..=20, "top" -> overall top runs; ":N"
/// overrides the default count.
#[cfg(feature = "wcl")]
fn level_spec(s: &str, default_count: usize) -> anyhow::Result<LevelSpec> {
    let (range, count) = match s.rsplit_once(':') {
        Some((r, n)) => (
            r,
            n.trim()
                .parse::<usize>()
                .with_context(|| format!("bad run count in --level {s:?}"))?,
        ),
        None => (s, default_count),
    };
    let parse = |x: &str| {
        x.trim()
            .parse::<u32>()
            .with_context(|| format!("bad key level {x:?} in --level {s:?}"))
    };
    let range = match range.trim() {
        "top" => None,
        r => Some(match r.split_once("..") {
            Some((a, b)) => (parse(a)?, parse(b)?),
            None => (parse(r)?, parse(r)?),
        }),
    };
    Ok(LevelSpec { range, count })
}

#[cfg(all(test, feature = "wcl"))]
mod tests {
    use super::*;

    #[test]
    fn level_specs() {
        let s = |x| level_spec(x, 5).unwrap();
        assert_eq!(
            s("12..16"),
            LevelSpec {
                range: Some((12, 16)),
                count: 5
            }
        );
        assert_eq!(
            s("20"),
            LevelSpec {
                range: Some((20, 20)),
                count: 5
            }
        );
        assert_eq!(
            s("top:3"),
            LevelSpec {
                range: None,
                count: 3
            }
        );
        assert_eq!(
            s("12..16:2"),
            LevelSpec {
                range: Some((12, 16)),
                count: 2
            }
        );
        assert!(level_spec("x..y", 5).is_err());
        assert!(level_spec("top:many", 5).is_err());
    }
}

fn season_suggest(a: SuggestArgs) -> anyhow::Result<()> {
    use wowcoach_core::suggest::{self, Collected, Origin};
    let previous = std::fs::read_to_string(&a.pack)
        .with_context(|| format!("reading {}", a.pack.display()))?;
    let mut pack = SeasonPack::parse(&previous)
        .with_context(|| format!("{} is not a valid pack", a.pack.display()))?;
    let mut c = Collected::default();

    for log in &a.logs {
        let file =
            std::fs::File::open(log).with_context(|| format!("opening {}", log.display()))?;
        let cfg = Config::load_with(None, PackChoice::None)?;
        let (_, fights, _) =
            wowcoach_core::analyze_log(std::io::BufReader::with_capacity(1 << 20, file), cfg)?;
        eprintln!("{}: {} fight(s)", log.display(), fights.len());
        for f in &fights {
            c.add(&pack, f, Origin::Own);
        }
    }

    #[cfg(feature = "wcl")]
    suggest_from_wcl(&a, &pack, &mut c)?;

    if c.fights_used.is_empty() && c.fights_skipped.is_empty() {
        anyhow::bail!("nothing to learn from: pass logs, --wcl reports or --wcl-top N");
    }
    let applied = suggest::apply(&mut pack, &c);
    let problems = pack.problems();
    if !problems.is_empty() {
        anyhow::bail!(
            "the updated pack is invalid, nothing written:\n  - {}",
            problems.join("\n  - ")
        );
    }
    print!("{}", suggest::review(&c, &applied));
    println!("{}", pack.summary());
    let text = wowcoach_core::season::render(&pack, &previous)?;
    if !a.write {
        println!("Review only: pass --write to update {}.", a.pack.display());
    } else if text == previous {
        println!("{} is already up to date.", a.pack.display());
    } else {
        std::fs::write(&a.pack, text).with_context(|| format!("writing {}", a.pack.display()))?;
        println!(
            "Wrote {}. Promote suggestions to `tags` (or move them to `dismissed`) in review.",
            a.pack.display()
        );
    }
    Ok(())
}

#[cfg(feature = "wcl")]
fn suggest_from_wcl(
    a: &SuggestArgs,
    pack: &SeasonPack,
    c: &mut wowcoach_core::suggest::Collected,
) -> anyhow::Result<()> {
    use wowcoach_core::suggest::{self, Origin};
    if a.wcl.is_empty() && a.wcl_top == 0 && a.levels.iter().all(|l| !l.contains(':')) {
        return Ok(());
    }
    let (Ok(id), Ok(secret)) = (
        std::env::var("WCL_CLIENT_ID"),
        std::env::var("WCL_CLIENT_SECRET"),
    ) else {
        anyhow::bail!("--wcl / --wcl-top need WCL_CLIENT_ID and WCL_CLIENT_SECRET (env or ./.env)");
    };
    let cache = a.cache_dir.clone().unwrap_or_else(|| {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("wowcoach/suggest")
    });
    eprintln!(
        "Connecting to Warcraft Logs (evidence cache: {})...",
        cache.display()
    );
    let client = wcl::net::Client::connect(&id, &secret)?;
    let (spent0, limit, _) = client.rate_limit()?;
    // Stop at the run budget, and always leave 10% of the hourly limit.
    let ceiling = (spent0 + a.max_points).min(limit * 0.9);

    let analyse = |code: &str, fights: &[i64]| -> anyhow::Result<Vec<analysis::FightReport>> {
        let fetched = wcl::net::fetch(&client, code, fights, false)?;
        let mut an = Analyzer::new(Config::load_with(None, PackChoice::None)?);
        for e in fetched.events {
            an.feed(e);
        }
        Ok(an.finish().0)
    };

    for spec in &a.wcl {
        let (code, fights) = match spec.split_once(':') {
            Some((code, f)) => (
                code,
                vec![
                    f.parse::<i64>()
                        .with_context(|| format!("bad fight ID in {spec:?}"))?,
                ],
            ),
            None => (spec.as_str(), vec![]),
        };
        for f in analyse(code, &fights)? {
            c.add(pack, &f, Origin::Own);
        }
    }

    let specs: Vec<LevelSpec> = if a.levels.is_empty() {
        vec![LevelSpec {
            range: None,
            count: a.wcl_top,
        }]
    } else {
        a.levels
            .iter()
            .map(|l| level_spec(l, a.wcl_top))
            .collect::<anyhow::Result<_>>()?
    };
    if specs.iter().any(|s| s.count > 0) {
        let encounters = client.zone_encounters()?;
        'dungeons: for d in &pack.dungeons {
            let Some((_, enc)) = encounters
                .iter()
                .find(|(n, id)| n.eq_ignore_ascii_case(&d.name) && *id % 10_000 == d.map_id)
            else {
                eprintln!("  {}: not found on Warcraft Logs, skipped", d.name);
                continue;
            };
            let mut keys: Vec<wcl::RankedKey> = vec![];
            for spec in &specs {
                // Highest level first; "top" = the overall best runs.
                let (lo, hi) = spec.range.unwrap_or((0, u32::MAX));
                let levels: Vec<Option<u32>> = match spec.range {
                    None => vec![None],
                    Some(_) => (lo.max(2)..=hi.min(40)).rev().map(Some).collect(),
                };
                let mut got = 0;
                'levels: for level in levels {
                    for page in 1..=3 {
                        let (ranked, more) = client.top_keys(*enc, level, page)?;
                        for k in ranked {
                            if got < spec.count
                                && (lo..=hi).contains(&k.level)
                                && !keys
                                    .iter()
                                    .any(|x| x.code == k.code && x.fight_id == k.fight_id)
                            {
                                keys.push(k);
                                got += 1;
                            }
                        }
                        if got >= spec.count {
                            break 'levels;
                        }
                        if !more {
                            break;
                        }
                    }
                }
            }
            eprintln!(
                "  {}: {} public run(s) at levels {}",
                d.name,
                keys.len(),
                keys.iter()
                    .map(|k| format!("+{}", k.level))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            for k in &keys {
                // Public runs don't change: reuse their analysed evidence.
                let cached = cache.join(format!(
                    "v{}-{}-{}.json",
                    suggest::EVIDENCE_VERSION,
                    k.code,
                    k.fight_id
                ));
                if let Some(fights) = std::fs::read_to_string(&cached)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Vec<suggest::FightEvidence>>(&t).ok())
                {
                    for f in &fights {
                        c.add_evidence(pack, f, Origin::Top);
                    }
                    continue;
                }
                let (spent, _, reset) = client.rate_limit()?;
                if spent + 60.0 > ceiling {
                    eprintln!(
                        "  Stopping: {spent:.0} of {limit:.0} Warcraft Logs points used this hour (budget {:.0}); resets in {} min. Re-run later: analysed runs are cached.",
                        ceiling,
                        reset / 60
                    );
                    break 'dungeons;
                }
                match analyse(&k.code, &[k.fight_id]) {
                    Ok(fights) => {
                        let ev: Vec<suggest::FightEvidence> =
                            fights.iter().map(Into::into).collect();
                        for f in &ev {
                            c.add_evidence(pack, f, Origin::Top);
                        }
                        std::fs::create_dir_all(&cache)?;
                        std::fs::write(&cached, serde_json::to_string(&ev)?)?;
                    }
                    Err(e) => eprintln!("    skipped {}:{}: {e:#}", k.code, k.fight_id),
                }
            }
        }
        let (spent, _, _) = client.rate_limit()?;
        eprintln!("Warcraft Logs points used: {:.0}", spent - spent0);
    }
    Ok(())
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
    /// Class abilities: interrupts, cast-stopping CC, defensives, dispels
    Classes {
        #[command(subcommand)]
        cmd: ClassesCmd,
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
        Cmd::Season {
            cmd: SeasonCmd::Suggest(args),
        } => season_suggest(args)?,
        Cmd::Classes {
            cmd:
                ClassesCmd::Check {
                    logs,
                    table,
                    #[cfg(feature = "blizzard")]
                    live,
                    #[cfg(feature = "blizzard")]
                    region,
                },
        } => {
            #[cfg(feature = "blizzard")]
            let live = live.then_some(region.as_str());
            #[cfg(not(feature = "blizzard"))]
            let live = None;
            classes_check(table, &logs, live)?
        }
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
