//! Golden snapshot: the full JSON report for the synthetic M+ log must not
//! change unless the change is intended. `tools/gen_sample_log.py` bakes known
//! problems into `samples/mplus.txt`, so any diff here is a behaviour change.
//!
//! After an intended change, refresh the snapshot and review the diff:
//!
//!     WOWCOACH_BLESS=1 cargo test -p wowcoach-core --test golden

use std::path::{Path, PathBuf};
use wowcoach_core::config::Config;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn check(sample: &str, config: &str, golden: &str) {
    let root = repo_root();
    let cfg = Config::load(Some(&root.join(config))).expect("load config");
    let file = std::fs::File::open(root.join(sample)).expect("open sample log");
    let (_, fights, cfg) =
        wowcoach_core::analyze_log(std::io::BufReader::new(file), cfg).expect("parse");
    let report = wowcoach_core::build_report(sample.to_string(), fights, &cfg);
    let actual = serde_json::to_string_pretty(&report).unwrap() + "\n";

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
