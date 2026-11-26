//! The built-in season pack on a real key. `fixtures/den-of-nalorakk-excerpt.txt`
//! is an anonymised excerpt of a +13 Den of Nalorakk (tools/anonymise_log.py):
//! the key's first lines (start and specs) plus a pull where Winter's Shroud, a
//! priority kick in the pack, was kicked twice and went off once.

use std::path::Path;
use wowcoach_core::analysis::FightReport;
use wowcoach_core::config::{Config, PackChoice};

fn run(pack: PackChoice) -> FightReport {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/den-of-nalorakk-excerpt.txt");
    let file = std::fs::File::open(path).unwrap();
    let cfg = Config::load_with(None, pack).unwrap();
    let (_, fights, cfg) = wowcoach_core::analyze_log(std::io::BufReader::new(file), cfg).unwrap();
    let mut r = wowcoach_core::build_report("fixture".into(), fights, &cfg);
    assert_eq!(r.fights.len(), 1);
    r.fights.remove(0)
}

#[test]
fn embedded_pack_marks_priority_kicks_on_a_real_key() {
    let f = run(PackChoice::Embedded);
    assert!(f.title.contains("Den of Nalorakk"), "{}", f.title);
    let shroud = f
        .enemy_casts
        .iter()
        .find(|c| c.name == "Winter's Shroud")
        .unwrap();
    assert_eq!(
        (shroud.interrupted, shroud.succeeded, shroud.priority),
        (2, 1, true)
    );
    let kicks = f
        .suggestions
        .iter()
        .find(|s| s.scope == "group" && s.category == "Interrupts")
        .expect("an interrupt suggestion");
    assert!(
        kicks
            .message
            .contains("Winter's Shroud (1 went off, 2 kicked, PRIORITY)"),
        "{}",
        kicks.message
    );
    assert_eq!(
        format!("{:?}", kicks.severity),
        "High",
        "a priority cast went off"
    );
}

#[test]
fn without_the_pack_the_same_cast_is_not_priority() {
    let f = run(PackChoice::None);
    let shroud = f
        .enemy_casts
        .iter()
        .find(|c| c.name == "Winter's Shroud")
        .unwrap();
    assert!(!shroud.priority);
}

#[test]
fn every_player_spec_has_class_abilities() {
    // The specs logged in a real key all resolve to class-table entries.
    use wowcoach_core::classes::{AbilityKind, ClassData};
    let classes = ClassData::embedded();
    let f = run(PackChoice::Embedded);
    assert_eq!(f.players.len(), 5);
    for p in &f.players {
        assert!(!p.role_inferred, "{} has no logged spec", p.name);
        let spec = wowcoach_core::specs::SPECS
            .iter()
            .find(|s| s.1 == p.class && s.2 == p.spec)
            .unwrap();
        assert!(
            !classes.for_spec(spec.0, AbilityKind::Defensive).is_empty(),
            "{} {}",
            p.spec,
            p.class
        );
    }
}

#[test]
fn cast_records_on_a_real_key() {
    use wowcoach_core::analysis::CastOutcome;
    let f = run(PackChoice::Embedded);
    let shroud: Vec<_> = f
        .casts
        .iter()
        .filter(|c| c.name == "Winter's Shroud")
        .collect();
    let kicked: Vec<&str> = shroud
        .iter()
        .filter_map(|c| match &c.outcome {
            CastOutcome::Kicked { by, .. } => Some(by.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(kicked.len(), 2, "{shroud:?}");
    assert!(
        kicked
            .iter()
            .all(|by| by.starts_with("Player") && by.ends_with("-Realm-US")),
        "{kicked:?}"
    );
    let done = shroud
        .iter()
        .find(|c| c.outcome == CastOutcome::Completed)
        .expect("one went off");
    assert!(done.priority);
    // Raw logs carry fresh positions: every ready kicker has a distance.
    assert!(
        done.ready_kickers.iter().all(|k| k.distance_yd.is_some()),
        "{:?}",
        done.ready_kickers
    );
}
