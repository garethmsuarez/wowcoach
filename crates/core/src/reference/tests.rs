use super::*;

fn pull(start_s: i64, end_s: i64, enc: u32) -> RefPull {
    RefPull {
        start_ms: start_s * 1000,
        end_ms: end_s * 1000,
        encounter_id: enc,
        kill: true,
    }
}

/// Trash 0–100 s (kills 2 × NPC 7), boss A (enc 1) 150–200 s, trash
/// 250–300 s (kills 1 × NPC 7), boss B (enc 2) 300–`b_end` s.
fn run(code: &str, b_end: i64) -> RefRun {
    RefRun {
        code: code.into(),
        fight_id: 1,
        map_id: 2649,
        level: 10,
        affixes: vec![],
        time_ms: b_end * 1000,
        bonus: Some(1),
        pulls: vec![
            pull(0, 100, 0),
            pull(150, 200, 1),
            pull(250, 300, 0),
            pull(300, b_end, 2),
        ],
        kills: vec![(50_000, 7), (90_000, 7), (280_000, 7), (290_000, 99)],
        non_tanks: 0,
        damage: vec![],
    }
}

fn boss(name: &str, enc: u32, end_s: f64, success: bool) -> BossMarker {
    BossMarker {
        name: name.into(),
        encounter_id: enc,
        start_s: end_s - 50.0,
        end_s,
        success,
        lust_ready: false,
        lust_used: false,
        forces_pct: None,
    }
}

fn seg(index: usize, boss: Option<&str>) -> Segment {
    Segment {
        index,
        boss: boss.map(Into::into),
        ..Default::default()
    }
}

#[test]
fn stat_median_and_range() {
    assert_eq!(Stat::of(vec![]), None);
    assert_eq!(
        Stat::of(vec![3.0, 1.0, 2.0]),
        Some(Stat {
            median: 2.0,
            min: 1.0,
            max: 3.0
        })
    );
    assert_eq!(Stat::of(vec![4.0, 1.0]).unwrap().median, 2.5);
}

#[test]
fn runs_line_up_with_your_bosses_and_segments() {
    let runs = [run("a", 400), run("b", 500), run("c", 600)];
    let table: HashMap<u32, u32> = [(7, 10)].into();
    // You killed A, wiped on a boss nobody else fought, then killed B, and
    // pulled trash after B.
    let bosses = [
        boss("A", 1, 210.0, true),
        boss("X", 9, 260.0, false),
        boss("B", 2, 450.0, true),
    ];
    let segments = [seg(1, Some("A")), seg(2, Some("B")), seg(3, None)];
    let t = summarize(
        &runs,
        10,
        &bosses,
        &segments,
        Some((40, &table)),
        &HashMap::new(),
    )
    .unwrap();

    assert_eq!(t.runs, ["a:1", "b:1", "c:1"]);
    assert_eq!(
        t.time_s,
        Stat {
            median: 500.0,
            min: 400.0,
            max: 600.0
        }
    );
    // 3 kills × 10 of 40; NPC 99 isn't in the table.
    assert_eq!(t.forces_pct.unwrap().median, 75.0);

    let b: Vec<(&str, f64, f64)> = t
        .bosses
        .iter()
        .map(|b| {
            (
                b.name.as_str(),
                b.kill_s.median,
                b.forces_pct.unwrap().median,
            )
        })
        .collect();
    assert_eq!(b, [("A", 200.0, 50.0), ("B", 500.0, 75.0)]);

    // Segment 1: 0–200 s, in combat 100 + 50 s. Segment 2: 200 s to B's
    // kill. Nobody pulled after B, so there's no trailing segment for them.
    assert_eq!(t.segments.len(), 2);
    let s1 = &t.segments[0];
    assert_eq!(
        (
            s1.duration_s.median,
            s1.combat_s.median,
            s1.downtime_s.median
        ),
        (200.0, 150.0, 50.0)
    );
    assert_eq!(s1.forces_pct.unwrap().median, 50.0);
    let s2 = &t.segments[1];
    assert_eq!(
        s2.duration_s,
        Stat {
            median: 300.0,
            min: 200.0,
            max: 400.0
        }
    );
    assert_eq!(s2.combat_s.median, 250.0);
    assert_eq!(s2.forces_pct.unwrap().median, 25.0);
}

#[test]
fn without_forces_or_runs() {
    let bosses = [boss("A", 1, 210.0, true)];
    let segments = [seg(1, Some("A"))];
    let t = summarize(
        &[run("a", 400)],
        10,
        &bosses,
        &segments,
        None,
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(t.forces_pct, None);
    assert_eq!(t.bosses[0].forces_pct, None);
    assert_eq!(t.segments[0].forces_pct, None);
    assert!(summarize(&[], 10, &bosses, &segments, None, &HashMap::new()).is_none());
    // A boss the runs never killed is left out.
    let other = [boss("Z", 5, 100.0, true)];
    let t = summarize(
        &[run("a", 400)],
        10,
        &other,
        &[seg(1, Some("Z"))],
        None,
        &HashMap::new(),
    )
    .unwrap();
    assert!(t.bosses.is_empty() && t.segments.is_empty());
}

#[test]
fn damage_per_player_per_ten_minutes_merged_by_name() {
    // Run a: 400 s of which 300 s in combat (pull windows), 4 non-tanks.
    // Run b: 500 s, 400 s in combat, 4 non-tanks, never hit by the spell.
    // Run c has no damage data and is left out.
    let mut a = run("a", 400);
    a.non_tanks = 4;
    a.damage = vec![
        RefDamage {
            spell_id: 10,
            hits: 8,
            amount: 800_000,
        },
        RefDamage {
            spell_id: 11,
            hits: 4,
            amount: 400_000,
        },
        RefDamage {
            spell_id: 99,
            hits: 50,
            amount: 9_000_000,
        },
    ];
    let mut b = run("b", 500);
    b.non_tanks = 4;
    let spells: HashMap<u32, String> =
        [(10, "Quake".to_string()), (11, "Quake".to_string())].into();
    let bosses = [boss("A", 1, 210.0, true)];
    let t = summarize(
        &[a, b, run("c", 600)],
        10,
        &bosses,
        &[seg(1, Some("A"))],
        None,
        &spells,
    )
    .unwrap();
    assert_eq!(t.damage_runs, 2);
    assert_eq!(t.damage.len(), 1, "only spells you took");
    let q = &t.damage[0];
    assert_eq!(q.name, "Quake");
    // 1.2M over 4 players and half of 10 minutes = 600K; run b 0.
    assert_eq!(
        (q.damage.max, q.damage.min, q.damage.median),
        (600_000.0, 0.0, 300_000.0)
    );
    assert_eq!(q.hits.max, 6.0);
}
