//! Rules are tested on hand-built reports, so each test states exactly the
//! metrics a rule reads and nothing else.

use super::*;
use crate::config::SpellNote;

fn player(name: &str, role: Role) -> PlayerReport {
    PlayerReport {
        name: name.into(),
        guid: format!("Player-1-{name}"),
        class: "Test".into(),
        spec: "Test".into(),
        role,
        role_inferred: false,
        damage_done: 0,
        dps: 1000.0,
        healing_done: 0,
        absorbs_done: 0,
        hps: 0.0,
        overheal_pct: 0.0,
        damage_taken: 0,
        avoidable_taken: 0,
        likely_avoidable_taken: 0,
        interrupts: 1,
        dispels: 0,
        deaths: 0,
        casts: 100,
        activity_pct: 0.9,
        healthstones: 0,
        potions: 0,
        top_damage_taken: vec![],
        wcl_rank_percent: None,
        kick_latency_s: None,
        priority_missed: 0,
        wasted_kicks: 0,
    }
}

fn party() -> Vec<PlayerReport> {
    vec![
        player("Tank", Role::Tank),
        player("Healer", Role::Healer),
        player("Alpha", Role::Dps),
        player("Bravo", Role::Dps),
        player("Charlie", Role::Dps),
    ]
}

fn fight(kind: FightKind, duration_s: f64) -> FightReport {
    FightReport {
        start_ts: 0,
        title: "test".into(),
        kind,
        duration_s,
        complete: true,
        players: party(),
        deaths: vec![],
        pulls: vec![],
        downtime_s: 0.0,
        enemy_casts: vec![],
        enemy_damage: vec![],
        lust_at_s: vec![0.0],
        suggestions: vec![],
        spell_evidence: vec![],
        casts: vec![],
    }
}

fn mplus(success: bool, duration_s: f64) -> FightReport {
    fight(
        FightKind::MythicPlus {
            zone: "Spire".into(),
            map_id: 2000,
            level: 10,
            affixes: vec![],
            success,
            time_s: duration_s,
            bosses: vec![],
        },
        duration_s,
    )
}

fn raid(success: bool, duration_s: f64, boss_hp_pct: Option<f64>) -> FightReport {
    fight(
        FightKind::Raid {
            encounter_id: 3001,
            name: "Boss".into(),
            difficulty: "Heroic".into(),
            group_size: 20,
            success,
            boss_hp_pct,
        },
        duration_s,
    )
}

fn death(player: &str, at_s: f64, spell: &str, spell_id: u32) -> DeathReport {
    DeathReport {
        player: player.into(),
        at_s,
        killing_blow: format!("{spell} (Boss)"),
        killing_spell_id: spell_id,
        recap: vec![],
        damage_last_5s: 0,
        healthstone_used_before: false,
        potion_used_before: false,
        pull: Some(1),
        wipe_cascade: false,
    }
}

fn pull(index: usize, start_s: f64, end_s: f64, deaths: u32) -> PullReport {
    PullReport {
        index,
        start_s,
        end_s,
        deaths,
        enemies_killed: 0,
        boss: None,
        casts_kicked: 0,
        casts_stopped: 0,
        casters_killed: 0,
        priority_completed: 0,
    }
}

fn run(mut f: FightReport, cfg: &Config) -> Vec<Suggestion> {
    apply(&mut f, cfg);
    f.suggestions
}

fn get<'a>(s: &'a [Suggestion], scope: &str, category: &str) -> Option<&'a Suggestion> {
    s.iter()
        .find(|x| x.scope == scope && x.category == category)
}

fn note(id: u32, name: &str, text: &str) -> SpellNote {
    SpellNote {
        spell_id: id,
        name: name.into(),
        note: text.into(),
    }
}

// ----------------------------------------------------------------- group

#[test]
fn quiet_fight_has_no_suggestions() {
    assert!(run(mplus(true, 1200.0), &Config::default()).is_empty());
    assert!(run(raid(true, 200.0, Some(0.0)), &Config::default()).is_empty());
}

#[test]
fn group_deaths_cost_timer_in_mplus() {
    let mut f = mplus(true, 1200.0);
    f.deaths = vec![
        death("Alpha-Realm", 10.0, "Void Pool", 1),
        death("Alpha-Realm", 20.0, "Void Pool", 1),
        death("Bravo-Realm", 30.0, "Slam", 2),
    ];
    let s = run(f, &Config::default());
    let d = get(&s, "group", "Deaths").unwrap();
    assert_eq!(d.severity, Severity::High);
    assert!(
        d.message
            .starts_with("3 death(s). Most common killing blows: Void Pool ×2, Slam ×1.")
    );
    assert!(d.message.contains("roughly 0:45 added"), "{}", d.message);
}

#[test]
fn early_raid_death_is_flagged_as_wipe_cause() {
    let mut f = raid(false, 300.0, Some(42.0));
    f.deaths = vec![death("Alpha-Realm", 60.0, "Cleave", 1)];
    let s = run(f, &Config::default());
    let w = get(&s, "group", "Wipe cause").unwrap();
    assert_eq!(w.severity, Severity::High);
    assert!(w.message.contains("1:00 into a 5:00 pull"));
    assert_eq!(w.evidence, "Boss ended at 42.0% HP.");

    // First death after early_death_frac (30%) of the pull: not "early".
    let mut f = raid(false, 300.0, Some(42.0));
    f.deaths = vec![death("Alpha-Realm", 200.0, "Cleave", 1)];
    assert!(get(&run(f, &Config::default()), "group", "Wipe cause").is_none());
}

fn missed_kicks(mut f: FightReport) -> FightReport {
    f.enemy_casts = vec![EnemyCastReport {
        spell_id: 77,
        name: "Shadow Volley".into(),
        started: 3,
        succeeded: 2,
        interrupted: 1,
        priority: true,
    }];
    for p in f.players.iter_mut() {
        p.interrupts = 0;
    }
    f.players[2].interrupts = 1; // Alpha kicked
    f
}

#[test]
fn missed_priority_kicks_in_mplus() {
    let s = run(missed_kicks(mplus(true, 1200.0)), &Config::default());
    let kicks: Vec<&Suggestion> = s
        .iter()
        .filter(|x| x.scope == "group" && x.category == "Interrupts")
        .collect();
    assert_eq!(kicks.len(), 2);
    assert_eq!(kicks[0].severity, Severity::High);
    assert!(
        kicks[0]
            .message
            .contains("Shadow Volley (2 went off, 1 kicked, PRIORITY)")
    );
    // Healers aren't expected to kick; Alpha did.
    assert!(
        kicks[1]
            .message
            .contains("while Tank, Bravo, Charlie interrupted nothing"),
        "{}",
        kicks[1].message
    );
    for who in ["Tank", "Bravo", "Charlie"] {
        assert!(get(&s, who, "Interrupts").is_some(), "{who}");
    }
    assert!(get(&s, "Healer", "Interrupts").is_none());
    assert!(get(&s, "Alpha", "Interrupts").is_none());
}

#[test]
fn raid_kicks_are_not_assigned_per_player() {
    let s = run(
        missed_kicks(raid(true, 200.0, Some(0.0))),
        &Config::default(),
    );
    let group: Vec<_> = s.iter().filter(|x| x.category == "Interrupts").collect();
    assert_eq!(group.len(), 1);
    assert_eq!(group[0].scope, "group");
}

#[test]
fn avoidable_damage_group_and_player() {
    let mut f = mplus(true, 1200.0);
    f.enemy_damage = vec![EnemySpellReport {
        spell_id: 88,
        name: "Void Pool".into(),
        total: 1_500_000,
        hits: 3,
        players_hit: 1,
        non_tank_hit_frac: 0.25,
        max_hits_per_player: 3,
        median_hits_per_player: 3.0,
        configured_avoidable: true,
        likely_avoidable: false,
        per_player: vec![("Charlie".into(), 1_500_000, 3)],
    }];
    f.players[4].avoidable_taken = 1_500_000;
    let s = run(f, &Config::default());
    assert_eq!(
        get(&s, "group", "Avoidable damage").unwrap().message,
        "The group took avoidable damage from: Void Pool (1.50M over 3 hits)."
    );
    let p = get(&s, "Charlie", "Avoidable damage").unwrap();
    assert_eq!(p.severity, Severity::High);
    assert!(get(&s, "Bravo", "Avoidable damage").is_none());
}

#[test]
fn bloodlust_missing_or_late_on_raid_kills() {
    let mut f = raid(true, 120.0, Some(0.0));
    f.lust_at_s = vec![];
    let s = run(f, &Config::default());
    assert_eq!(
        get(&s, "group", "Cooldowns").unwrap().severity,
        Severity::Medium
    );

    let mut f = raid(true, 200.0, Some(0.0));
    f.lust_at_s = vec![150.0];
    let s = run(f, &Config::default());
    let late = get(&s, "group", "Cooldowns").unwrap();
    assert_eq!(late.severity, Severity::Low);
    assert!(late.message.contains("2:30 on a 3:20 kill"));

    // Wipes and short kills are left alone.
    let mut f = raid(false, 200.0, Some(10.0));
    f.lust_at_s = vec![];
    assert!(get(&run(f, &Config::default()), "group", "Cooldowns").is_none());
}

#[test]
fn mplus_pacing_pull_planning_and_timer() {
    let mut f = mplus(false, 600.0);
    f.downtime_s = 200.0;
    f.pulls = vec![
        pull(1, 0.0, 100.0, 0),
        pull(2, 220.0, 300.0, 3),
        pull(3, 330.0, 600.0, 0),
    ];
    let s = run(f, &Config::default());
    let pacing = get(&s, "group", "Pacing").unwrap();
    assert_eq!(pacing.severity, Severity::High); // depleted
    assert!(pacing.message.starts_with("33% of the run (3:20)"));
    assert!(
        pacing
            .message
            .contains("Longest gaps: 2:00 gap at 1:40, 0:30 gap at 5:00.")
    );
    assert!(
        get(&s, "group", "Pull planning")
            .unwrap()
            .message
            .contains("pull 2 at 3:40 (3 deaths)")
    );
    assert_eq!(
        get(&s, "group", "Timer").unwrap().message,
        "Key not timed (10:00)."
    );
}

// ---------------------------------------------------------------- player

#[test]
fn player_death_to_avoidable_mechanic() {
    let cfg = Config {
        avoidable: vec![note(88, "Void Pool", "Don't stand in the purple pools")],
        ..Config::default()
    };
    let mut f = mplus(true, 1200.0);
    f.deaths = vec![
        death("Alpha", 30.0, "Void Pool", 88),
        death("Bravo", 40.0, "Slam", 2),
    ];
    let s = run(f, &cfg);
    let a = get(&s, "Alpha", "Death").unwrap();
    assert_eq!(a.severity, Severity::High);
    assert_eq!(
        a.message,
        "Died at 0:30 to Void Pool (Boss). That's an avoidable mechanic: Don't stand in the purple pools."
    );
    let b = get(&s, "Bravo", "Death").unwrap();
    assert_eq!(b.severity, Severity::Medium);
    assert!(b.message.contains("No healthstone or potion was used"));
}

#[test]
fn wipe_cascade_deaths_are_ignored() {
    let mut f = raid(false, 300.0, Some(5.0));
    let mut d = death("Alpha", 290.0, "Enrage", 9);
    d.wipe_cascade = true;
    f.deaths = vec![d];
    let s = run(f, &Config::default());
    assert!(get(&s, "Alpha", "Death").is_none());
    assert!(get(&s, "group", "Deaths").is_none());
}

#[test]
fn low_activity_and_output() {
    let mut f = mplus(true, 1200.0);
    f.players[4].activity_pct = 0.5; // Charlie
    f.players[4].dps = 500.0;
    f.players[0].activity_pct = 0.1; // tanks aren't judged on activity
    let s = run(f, &Config::default());
    let act = get(&s, "Charlie", "Activity").unwrap();
    assert_eq!(act.severity, Severity::Medium);
    assert!(act.message.contains("was 50% (group median 90%)"));
    let out = get(&s, "Charlie", "Output").unwrap();
    assert!(out.message.contains("50% of the group DPS median"));
    assert!(out.message.contains("Likely contributors: 50% activity."));
    assert!(get(&s, "Tank", "Activity").is_none());
    assert!(get(&s, "Bravo", "Output").is_none());
}

#[test]
fn healer_overheal_and_wcl_rank() {
    let mut f = raid(true, 200.0, Some(0.0));
    f.players[1].healing_done = 1_000_000;
    f.players[1].overheal_pct = 0.6;
    f.players[2].wcl_rank_percent = Some(12.0);
    let s = run(f, &Config::default());
    assert!(
        get(&s, "Healer", "Healing efficiency")
            .unwrap()
            .message
            .starts_with("60% overhealing.")
    );
    assert_eq!(
        get(&s, "Alpha", "Benchmark").unwrap().message,
        "Warcraft Logs ranks this parse at 12 for the spec on this encounter."
    );
}

#[test]
fn suggestions_sorted_by_severity_then_scope() {
    let mut f = mplus(false, 1200.0);
    f.deaths = vec![
        death("Bravo", 10.0, "Slam", 2),
        death("Alpha", 20.0, "Slam", 2),
    ];
    let s = run(f, &Config::default());
    let order: Vec<(Severity, &str)> = s.iter().map(|x| (x.severity, x.scope.as_str())).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert_eq!(order[0], (Severity::Medium, "Alpha"));
}

// ----------------------------------------------------------- progression

#[test]
fn progression_advice() {
    let mut p = ProgressionReport {
        encounter_id: 3001,
        name: "Boss".into(),
        difficulty: "Mythic".into(),
        attempts: 5,
        kills: 0,
        best_hp_pct: Some(12.3),
        avg_duration_s: 185.0,
        death_causes: vec![],
        first_death_causes: vec![("Cleave".into(), 3)],
        frequent_deaths: vec![("Alpha".into(), 3)],
        suggestions: vec![],
    };
    apply_progression(&mut p);
    let s = &p.suggestions;
    assert_eq!(s.len(), 3);
    assert_eq!(
        s[0].message,
        "Cleave caused the first death in 3 of 5 pulls. Solve it before anything else."
    );
    assert_eq!(
        (s[1].scope.as_str(), s[1].severity),
        ("Alpha", Severity::Medium)
    );
    assert_eq!(
        s[2].message,
        "5 pulls so far, best at 12.3%. Average pull length: 3:05."
    );

    // One-off first-death causes and kills don't trigger advice.
    p.first_death_causes = vec![("Cleave".into(), 1)];
    p.frequent_deaths = vec![("Alpha".into(), 1)];
    p.kills = 1;
    apply_progression(&mut p);
    assert!(p.suggestions.is_empty());
}

// ------------------------------------------------------------ kick rules

fn cast(name: &str, start: f64, end: f64, priority: bool, outcome: CastOutcome) -> CastRecord {
    CastRecord {
        spell_id: 1,
        name: name.into(),
        caster: "Caster".into(),
        caster_npc: 1,
        start_s: start,
        end_s: end,
        pull: Some(1),
        priority,
        outcome,
        ready_kickers: vec![],
    }
}

fn ready(player: &str, ability: &str, d: f32, in_range: bool) -> ReadyKicker {
    ReadyKicker {
        player: player.into(),
        ability: ability.into(),
        distance_yd: Some(d),
        in_range: Some(in_range),
    }
}

fn kick_fight() -> FightReport {
    let mut f = mplus(true, 1200.0);
    let mut a = cast("Healing Breeze", 10.0, 12.5, true, CastOutcome::Completed);
    a.ready_kickers = vec![
        ready("Alpha", "Pummel", 4.0, true),
        ready("Bravo", "Counterspell", 60.0, false),
    ];
    let b = cast("Arc Lightning", 30.0, 32.0, true, CastOutcome::Completed);
    let filler = cast(
        "Earth Bolt",
        9.8,
        10.6,
        false,
        CastOutcome::Kicked {
            by: "Charlie".into(),
            spell: "Kick".into(),
            latency_s: 0.8,
        },
    );
    f.casts = vec![filler, a, b];
    f
}

#[test]
fn group_priority_kicks_name_who_was_ready() {
    let s = run(kick_fight(), &Config::default());
    let g = s
        .iter()
        .find(|x| x.scope == "group" && x.category == "Priority kicks")
        .unwrap();
    assert_eq!(g.severity, Severity::High);
    assert_eq!(
        g.message,
        "2 priority cast(s) went off. For 1 of them someone had a kick ready and in range. Most often ready: Alpha ×1. First: Healing Breeze at 0:12 (pull 1), with Alpha (Pummel, 4 yd) ready. For 1, nobody's kick was ready: those packs need more kickers or a stun."
    );
}

#[test]
fn players_get_missed_and_wasted_kicks() {
    let s = run(kick_fight(), &Config::default());
    let alpha = get(&s, "Alpha", "Priority kicks").unwrap();
    assert_eq!(
        alpha.message,
        "Pummel ready and in range for 1 priority cast(s) that went off: Healing Breeze at 0:12."
    );
    assert_eq!(alpha.evidence, "Distances when the casts finished: 4 yd.");
    assert!(get(&s, "Bravo", "Priority kicks").is_none(), "out of range");
    let charlie = get(&s, "Charlie", "Wasted kicks").unwrap();
    assert_eq!(
        charlie.message,
        "Kicked Earth Bolt at 0:10 while Healing Breeze was already casting; it went off at 0:12 with your kick on cooldown. Save the kick for the priority cast."
    );
}

#[test]
fn simultaneous_casts_are_merged_and_generic_advice_deduplicated() {
    let mut f = kick_fight();
    let mut twin = f.casts[1].clone();
    twin.caster = "Other Caster".into();
    f.casts.push(twin);
    // Alpha never kicked, but the specific advice already covers that.
    f.enemy_casts = vec![EnemyCastReport {
        spell_id: 1,
        name: "Healing Breeze".into(),
        started: 3,
        succeeded: 3,
        interrupted: 1,
        priority: true,
    }];
    for p in f.players.iter_mut() {
        p.interrupts = 0;
    }
    let s = run(f, &Config::default());
    assert!(
        get(&s, "Alpha", "Priority kicks")
            .unwrap()
            .message
            .contains("Healing Breeze at 0:12 (×2)")
    );
    assert!(
        get(&s, "Alpha", "Interrupts").is_none(),
        "no generic 'no interrupts' on top"
    );
    assert!(get(&s, "Bravo", "Interrupts").is_some());
}

#[test]
fn no_priority_kick_advice_in_raids_or_without_ready_kickers() {
    let mut r = raid(true, 300.0, Some(0.0));
    r.casts = kick_fight().casts;
    assert!(
        run(r, &Config::default())
            .iter()
            .all(|s| s.category != "Priority kicks")
    );

    let mut f = mplus(true, 1200.0);
    f.casts = vec![cast(
        "Arc Lightning",
        30.0,
        32.0,
        true,
        CastOutcome::Completed,
    )];
    let s = run(f, &Config::default());
    let g = get(&s, "group", "Priority kicks").unwrap();
    assert_eq!(g.severity, Severity::Medium);
    assert!(g.message.starts_with(
        "1 priority cast(s) went off. For 0 of them someone had a kick ready and in range."
    ));
}

#[test]
fn interrupt_list_puts_priority_casts_first() {
    let mut f = mplus(true, 1200.0);
    f.enemy_casts = vec![
        EnemyCastReport {
            spell_id: 1,
            name: "Earth Bolt".into(),
            started: 40,
            succeeded: 29,
            interrupted: 11,
            priority: false,
        },
        EnemyCastReport {
            spell_id: 2,
            name: "Healing Breeze".into(),
            started: 3,
            succeeded: 1,
            interrupted: 2,
            priority: true,
        },
    ];
    let s = run(f, &Config::default());
    let m = &get(&s, "group", "Interrupts").unwrap().message;
    assert!(
        m.find("Healing Breeze").unwrap() < m.find("Earth Bolt").unwrap(),
        "{m}"
    );
}
