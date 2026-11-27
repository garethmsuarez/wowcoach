//! Golden snapshot: the full JSON report for the synthetic M+ log must not
//! change unless the change is intended. `tools/gen_sample_log.py` bakes known
//! problems into `samples/mplus.txt`, so any diff here is a behaviour change.
//!
//! After an intended change, refresh the snapshot and review the diff:
//!
//!     WOWCOACH_BLESS=1 cargo test -p wowcoach-core --test golden

use std::path::{Path, PathBuf};
use wowcoach_core::config::{Config, PackChoice};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn report(sample: &str, config: &str, pack: PackChoice) -> wowcoach_core::report::Report {
    let root = repo_root();
    let cfg = Config::load_with(Some(&root.join(config)), pack).expect("load config");
    let file = std::fs::File::open(root.join(sample)).expect("open sample log");
    let (_, fights, cfg) =
        wowcoach_core::analyze_log(std::io::BufReader::new(file), cfg).expect("parse");
    wowcoach_core::build_report(sample.to_string(), fights, &cfg)
}

fn check(sample: &str, config: &str, golden: &str) {
    // No season pack: the synthetic log uses made-up spell IDs, and the snapshot
    // must not change whenever the real pack gains entries.
    let r = report(sample, config, PackChoice::None);
    compare(serde_json::to_string_pretty(&r).unwrap() + "\n", golden);
}

fn compare(actual: String, golden: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(golden);
    if std::env::var_os("WOWCOACH_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            panic!(
                "reading {}: {e}. Run with WOWCOACH_BLESS=1 to create it.",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    if expected == actual {
        return;
    }
    let (exp, act): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    let first = exp
        .iter()
        .zip(&act)
        .position(|(a, b)| a != b)
        .unwrap_or(exp.len().min(act.len()));
    let lo = first.saturating_sub(5);
    let ctx = |v: &[&str]| v[lo.min(v.len())..(first + 6).min(v.len())].join("\n");
    panic!(
        "{golden} differs from the snapshot at line {} ({} vs {} lines).\n--- expected\n{}\n--- actual\n{}\n\nIf this change is intended, rerun with WOWCOACH_BLESS=1 and review the diff.",
        first + 1,
        exp.len(),
        act.len(),
        ctx(&exp),
        ctx(&act)
    );
}

#[test]
fn mplus_sample_report() {
    check("samples/mplus.txt", "samples/wowcoach.toml", "mplus.json");
}

/// The route fields with a pack that has a timer and enemy forces for the
/// synthetic dungeon (tests/fixtures/synthetic-pack.toml): timed / upgrades,
/// kills and forces per pull, segments, and the Timer advice.
#[test]
fn mplus_route_with_forces() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-pack.toml");
    let r = report(
        "samples/mplus.txt",
        "samples/wowcoach.toml",
        PackChoice::File(pack),
    );
    let v = serde_json::to_value(&r).unwrap();
    let f = &v["fights"][0];
    let route = serde_json::json!({
        "title": f["title"],
        "kind": f["kind"],
        "forces_pct": f["forces_pct"],
        "pulls": f["pulls"].as_array().unwrap().iter().map(|p| serde_json::json!({
            "index": p["index"],
            "start_s": p["start_s"],
            "end_s": p["end_s"],
            "boss": p["boss"],
            "enemies_killed": p["enemies_killed"],
            "npcs": p["npcs"],
            "forces": p["forces"],
            "forces_pct": p["forces_pct"],
        })).collect::<Vec<_>>(),
        "segments": f["segments"],
        "timer": f["suggestions"].as_array().unwrap().iter()
            .filter(|s| s["category"] == "Timer").collect::<Vec<_>>(),
    });
    compare(
        serde_json::to_string_pretty(&route).unwrap() + "\n",
        "mplus-route.json",
    );
}

/// The same knowledge expressed as a season pack must coach exactly like the
/// older `[[avoidable]]` / `[[priority_interrupt]]` lists in samples/wowcoach.toml.
#[test]
fn season_pack_matches_legacy_config() {
    use wowcoach_core::season::{Knowledge, SeasonPack};
    let root = repo_root();
    let run = |cfg: Config| {
        let file = std::fs::File::open(root.join("samples/mplus.txt")).unwrap();
        let (_, fights, cfg) =
            wowcoach_core::analyze_log(std::io::BufReader::new(file), cfg).unwrap();
        let mut v =
            serde_json::to_value(wowcoach_core::build_report("x".into(), fights, &cfg)).unwrap();
        v["season_pack"] = serde_json::Value::Null;
        v
    };
    let legacy =
        Config::load_with(Some(&root.join("samples/wowcoach.toml")), PackChoice::None).unwrap();

    let pack = SeasonPack::parse(
        r#"
        [season]
        key = "synthetic"
        name = "Synthetic"
        [[spell]]
        ids = [1231001]
        name = "Dark Swirl"
        tags = ["avoidable"]
        note = "Step out of the swirl before it lands"
        [[spell]]
        ids = [88888]
        name = "Void Pool"
        tags = ["avoidable"]
        note = "Don't stand in the purple pools"
        [[spell]]
        ids = [77777]
        name = "Shadow Volley"
        tags = ["priority_kick"]
        [[spell]]
        ids = [99001]
        name = "Crushing Blow"
        tags = ["tank_buster"]
        [[spell]]
        ids = [99200]
        name = "Withering Hex"
        tags = ["dispel_priority"]
        dispel = ["magic"]
        "#,
    )
    .unwrap();
    let mut from_pack = Config::load_with(None, PackChoice::None).unwrap();
    from_pack.knowledge = Knowledge::build(Some(pack), &[]);

    let (a, b) = (run(legacy), run(from_pack));
    assert!(
        a == b,
        "pack-based report differs from the legacy-config report"
    );
    assert!(
        a.to_string().contains("PRIORITY"),
        "sanity: the priority kick was used"
    );
}
