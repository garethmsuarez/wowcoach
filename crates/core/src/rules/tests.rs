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
        priority_dispels: 0,
        dispel_latency_s: None,
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
        segments: vec![],
        forces_pct: None,
        reference: None,
        downtime_s: 0.0,
        enemy_casts: vec![],
        enemy_damage: vec![],
        lust_at_s: vec![0.0],
        suggestions: vec![],
        spell_evidence: vec![],
        casts: vec![],
        defensive_uses: vec![],
        spikes: vec![],
        tank_hits: vec![],
        enemy_summons: vec![],
        death_penalty_s: None,
        debuffs: vec![],
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
            timer_s: None,
            upgrades: None,
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
        ..Default::default()
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
        ..Default::default()
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
        kick: false,
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
fn avoidable_damage_merges_ids_by_name() {
    let mut f = mplus(true, 1200.0);
    let spell = |id: u32, name: &str, total: u64, hits: u32| EnemySpellReport {
        spell_id: id,
        name: name.into(),
        total,
        hits,
        players_hit: 1,
        non_tank_hit_frac: 0.25,
        max_hits_per_player: hits,
        median_hits_per_player: hits as f64,
        configured_avoidable: true,
        likely_avoidable: false,
        per_player: vec![],
    };
    f.enemy_damage = vec![
        spell(1, "Earthquake", 900_000, 3),
        spell(2, "Burn Corruption", 1_000_000, 2),
        spell(3, "Earthquake", 400_000, 1),
    ];
    let s = run(f, &Config::default());
    assert_eq!(
        get(&s, "group", "Avoidable damage").unwrap().message,
        "The group took avoidable damage from: Earthquake (1.30M over 4 hits); Burn Corruption (1.00M over 2 hits)."
    );
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
        "Key abandoned after 10:00."
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
        tier: priority.then_some(1),
        outcome,
        ready_kickers: vec![],
        ready_stoppers: vec![],
    }
}

fn ready(player: &str, ability: &str, d: f32, in_range: bool) -> ReadyKicker {
    ReadyKicker {
        player: player.into(),
        ability: ability.into(),
        distance_yd: Some(d),
        in_range: Some(in_range),
        cd_s: 15.0,
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

/// Three Earth Bolts (kick when free) went off with Alpha's kick ready and in
/// range; `breeze_at` adds a Healing Breeze (must kick) in the same pull.
fn free_kick_fight(breeze: Option<(f64, CastOutcome)>) -> FightReport {
    let mut f = mplus(true, 1200.0);
    for i in 0..3 {
        let at = 100.0 + 40.0 * i as f64;
        let mut c = cast("Earth Bolt", at, at + 2.0, false, CastOutcome::Completed);
        c.tier = Some(2);
        c.ready_kickers = vec![ready("Alpha", "Pummel", 5.0, true)];
        f.casts.push(c);
    }
    if let Some((at, outcome)) = breeze {
        let mut c = cast("Healing Breeze", at, at + 2.5, true, outcome);
        if c.outcome == CastOutcome::Completed {
            c.ready_kickers = vec![ready("Alpha", "Pummel", 5.0, true)];
        }
        f.casts.push(c);
    }
    f
}

#[test]
fn free_kicks_are_named_when_no_priority_cast_needs_the_kick() {
    let mut f = free_kick_fight(None);
    // Bravo was ready for the last one too: one group line, not one per player.
    f.casts[2]
        .ready_kickers
        .push(ready("Bravo", "Kick", 8.0, true));
    let s = run(f, &Config::default());
    let g = get(&s, "group", "Free kicks").unwrap();
    assert_eq!(g.severity, Severity::Low);
    assert_eq!(
        g.message,
        "3 kick-when-free cast(s) went off with a kick ready and in range, and no priority cast to save it for: Earth Bolt ×3. Most often ready: Alpha ×3, Bravo ×1."
    );
    assert!(get(&s, "Alpha", "Free kicks").is_none());
    assert!(
        get(&s, "Alpha", "Priority kicks").is_none(),
        "tier 2 is not a must-kick"
    );
    // A Healing Breeze elsewhere in the pull, outside the cooldown window of
    // every Earth Bolt, doesn't excuse them.
    let kicked = CastOutcome::Kicked {
        by: "Alpha".into(),
        spell: "Pummel".into(),
        latency_s: 1.0,
    };
    let s = run(free_kick_fight(Some((400.0, kicked))), &Config::default());
    assert!(get(&s, "group", "Free kicks").is_some());
}

#[test]
fn free_kicks_are_not_named_when_a_priority_cast_needed_the_kick() {
    // Healing Breeze right after the first Earth Bolt, kicked by Alpha: that
    // Earth Bolt was rightly let go, leaving 2, under the minimum of 3.
    let kicked = CastOutcome::Kicked {
        by: "Alpha".into(),
        spell: "Pummel".into(),
        latency_s: 1.0,
    };
    let f = free_kick_fight(Some((105.0, kicked)));
    assert_eq!(free_casts(&f.casts).len(), 2);
    assert!(get(&run(f, &Config::default()), "group", "Free kicks").is_none());
    // Same when the Healing Breeze went off with Alpha ready: Alpha should
    // have saved it, and that's Priority kicks advice instead.
    let f = free_kick_fight(Some((143.0, CastOutcome::Completed)));
    assert_eq!(free_casts(&f.casts).len(), 2);
    let s = run(f, &Config::default());
    assert!(get(&s, "group", "Free kicks").is_none());
    assert!(get(&s, "Alpha", "Priority kicks").is_some());
    // Someone else kicking the Breeze still counts: Alpha was right to hold
    // their kick as backup.
    let other = CastOutcome::Kicked {
        by: "Bravo".into(),
        spell: "Kick".into(),
        latency_s: 1.0,
    };
    assert_eq!(
        free_casts(&free_kick_fight(Some((105.0, other))).casts).len(),
        2
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
        kick: false,
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
            kick: false,
        },
        EnemyCastReport {
            spell_id: 2,
            name: "Healing Breeze".into(),
            started: 3,
            succeeded: 1,
            interrupted: 2,
            priority: true,
            kick: false,
        },
    ];
    let s = run(f, &Config::default());
    let m = &get(&s, "group", "Interrupts").unwrap().message;
    assert!(
        m.find("Healing Breeze").unwrap() < m.find("Earth Bolt").unwrap(),
        "{m}"
    );
}

/// For `rules::kickplan` tests: an M+/raid fight shell and a DPS player.
pub(crate) fn fight_for_plans(kind: FightKind) -> FightReport {
    fight(kind, 1500.0)
}

pub(crate) fn player_for_plans(name: &str) -> PlayerReport {
    player(name, Role::Dps)
}

#[test]
fn stoppable_priority_casts_with_a_stop_ready() {
    let mut f = mplus(true, 1200.0);
    let mut c = cast("Healing Breeze", 10.0, 12.5, true, CastOutcome::Completed);
    c.ready_stoppers = vec![
        ready("Monk", "Leg Sweep", 4.0, true),
        ready("Far", "Hex", 40.0, false),
    ];
    f.casts = vec![
        c,
        cast("Arc Lightning", 30.0, 32.0, true, CastOutcome::Completed),
    ];
    let s = run(f, &Config::default());
    let stops = get(&s, "group", "Stops").unwrap();
    assert_eq!(stops.severity, Severity::Medium);
    assert_eq!(
        stops.message,
        "1 priority cast(s) that crowd control also stops went off while a stop was ready: Healing Breeze at 0:12: Monk (Leg Sweep, 4 yd). When kicks are down, a stun or knock does the job."
    );
    assert!(
        run(mplus(true, 1200.0), &Config::default())
            .iter()
            .all(|x| x.category != "Stops")
    );
}

// ------------------------------------------------------------ defensives (M+)

fn rd(player: &str, ability: &str, last: f64) -> ReadyDefensive {
    ReadyDefensive {
        player: player.into(),
        ability: ability.into(),
        last_used_s: last,
    }
}

fn died_with(player: &str, at_s: f64, pull: usize, ready: Vec<ReadyDefensive>) -> DeathReport {
    DeathReport {
        pull: Some(pull),
        damage_last_5s_frac: Some(0.9),
        defensives_ready: ready,
        ..death(player, at_s, "Slam", 5)
    }
}

#[test]
fn deaths_with_a_defensive_ready() {
    let mut f = mplus(true, 1200.0);
    f.deaths = vec![
        died_with("Alpha", 100.0, 1, vec![rd("Alpha", "Blur", 30.0)]),
        // Not a burst death: a defensive wasn't the answer.
        DeathReport {
            damage_last_5s_frac: Some(0.1),
            ..died_with("Bravo", 110.0, 1, vec![rd("Bravo", "Barkskin", 30.0)])
        },
        died_with("Charlie", 120.0, 1, vec![]),
        // A wiping pull: only its first two deaths are judged.
        died_with("Tank", 300.0, 2, vec![rd("Tank", "Shield Wall", 10.0)]),
        died_with("Healer", 301.0, 2, vec![]),
        died_with("Alpha", 302.0, 2, vec![rd("Alpha", "Blur", 200.0)]),
        died_with("Bravo", 303.0, 2, vec![rd("Bravo", "Barkskin", 200.0)]),
    ];
    let s = run(f, &Config::default());
    let a = get(&s, "Alpha", "Defensives").unwrap();
    assert_eq!(
        a.message,
        "Died at 1:40 with Blur ready (last used at 0:30). Press it when your health starts dropping, not after."
    );
    assert_eq!(a.severity, Severity::Medium);
    assert_eq!(
        get(&s, "Tank", "Defensives").unwrap().message,
        "Died at 5:00 with Shield Wall ready (last used at 0:10). Press it when your health starts dropping, not after."
    );
    assert!(get(&s, "Bravo", "Defensives").is_none());
    assert!(get(&s, "Charlie", "Defensives").is_none());
    // Raid pulls keep their current rules until M7.
    let mut raid = fight(
        FightKind::Raid {
            encounter_id: 1,
            name: "Boss".into(),
            difficulty: "Mythic".into(),
            group_size: 20,
            success: false,
            boss_hp_pct: Some(50.0),
        },
        300.0,
    );
    raid.deaths = vec![died_with(
        "Alpha",
        100.0,
        1,
        vec![rd("Alpha", "Blur", 30.0)],
    )];
    assert!(get(&run(raid, &Config::default()), "Alpha", "Defensives").is_none());
}

fn spike(start: f64, peak: f64, top: &str, ready: Vec<ReadyDefensive>) -> Spike {
    Spike {
        pull: Some(1),
        start_s: start,
        end_s: start + 3.0,
        damage: 1,
        peak_frac: peak,
        top: vec![SpikeSpell {
            spell_id: 1,
            name: top.into(),
            source: "Brute".into(),
            source_npc: 1,
            damage: 1,
            summoned_by: None,
            summon_age_s: None,
        }],
        deaths: vec![],
        group_used: vec![],
        group_ready: ready,
    }
}

#[test]
fn spikes_with_a_group_defensive_ready() {
    let mut f = mplus(true, 1200.0);
    let answered = Spike {
        group_used: vec![GroupDefensive {
            player: "Tank".into(),
            ability: "Rallying Cry".into(),
            at_s: 50.0,
        }],
        ..spike(50.0, 0.9, "Slam", vec![rd("Healer", "Tranquility", 1.0)])
    };
    let led = Spike {
        top: vec![SpikeSpell {
            source: "Magma Totem".into(),
            summoned_by: Some("Shaman".into()),
            summon_age_s: Some(8.0),
            ..spike(0.0, 0.0, "Searing Magma", vec![]).top[0].clone()
        }],
        ..spike(
            400.0,
            0.95,
            "Searing Magma",
            vec![rd("Tank", "Rallying Cry", 1.0)],
        )
    };
    f.spikes = vec![
        spike(100.0, 0.55, "Volley", vec![rd("Tank", "Rallying Cry", 1.0)]),
        Spike {
            deaths: vec!["Alpha".into()],
            ..spike(200.0, 0.8, "Slam", vec![rd("Healer", "Tranquility", 1.0)])
        },
        answered,
        led,
    ];
    let s = run(f, &Config::default());
    let g = get(&s, "group", "Spikes").unwrap();
    assert_eq!(g.severity, Severity::High, "a death followed one");
    assert_eq!(
        g.message,
        "2 damage spike(s) went by without a group defensive while one was ready, worst first: 3:20 (pull 1, 80% of group HP, mostly Slam, 1 died): Tranquility (Healer); 1:40 (pull 1, 55% of group HP, mostly Volley): Rallying Cry (Tank). Agree before the key who covers which dangerous pull."
    );
    let mut none = mplus(true, 1200.0);
    none.spikes = vec![spike(100.0, 0.55, "Volley", vec![])];
    assert!(get(&run(none, &Config::default()), "group", "Spikes").is_none());
}

fn summon(name: &str, by: &str, start: f64, life: f64, killed: bool, dmg: u64) -> EnemySummon {
    EnemySummon {
        pull: Some(1),
        name: name.into(),
        npc: 1,
        summoner: by.into(),
        summoner_npc: 2,
        spell: "Summon".into(),
        start_s: start,
        end_s: start + life,
        killed,
        damage_to_group: dmg,
        healing_to_enemies: 0,
    }
}

#[test]
fn summons_that_lead_spikes_and_live_long() {
    let mut f = mplus(true, 1200.0);
    f.enemy_summons = vec![
        summon("Magma Totem", "Shaman", 10.0, 10.0, true, 1_000_000),
        summon("Magma Totem", "Shaman", 50.0, 12.0, true, 2_000_000),
        summon("Magma Totem", "Shaman", 90.0, 3.0, true, 100_000),
        summon("Magma Totem", "Shaman", 130.0, 90.0, false, 100_000),
        // Long-lived, but never behind a spike: cleave adds.
        summon("Hatchling", "Egg", 10.0, 30.0, true, 500_000),
        // Killed quickly: fine.
        summon("Volatile Totem", "Speaker", 10.0, 2.0, true, 400_000),
        // Decorative: no damage.
        summon("Spirit", "Shaman", 10.0, 99.0, false, 0),
    ];
    let mut s = spike(55.0, 0.7, "Searing Magma", vec![]);
    s.top[0].source = "Magma Totem".into();
    s.top[0].summoned_by = Some("Shaman".into());
    let mut v = spike(56.0, 0.7, "Searing Magma", vec![]);
    v.top[0].source = "Volatile Totem".into();
    v.top[0].summoned_by = Some("Speaker".into());
    f.spikes = vec![s, v];
    let out = run(f, &Config::default());
    let g = get(&out, "group", "Summons").unwrap();
    assert_eq!(g.severity, Severity::High);
    assert_eq!(
        g.message,
        "Enemy summons stayed up too long. Magma Totem (Shaman): 4 summoned, killed after a median 10 s (2 lived over 6 s, 1 not killed); 3.20M damage to the group, leading 1 damage spike(s). Kill them as soon as they drop."
    );
    // Spikes led by summons aren't defensive advice.
    assert!(get(&out, "group", "Spikes").is_none());
}

fn tank_hit(name: &str, at: f64, mitigation: &[&str]) -> TankHit {
    TankHit {
        pull: Some(1),
        at_s: at,
        tank: "Tank".into(),
        spell_id: 1,
        name: name.into(),
        school: 1,
        amount: 1,
        mitigation: mitigation.iter().map(|m| m.to_string()).collect(),
        defensives: vec![],
    }
}

#[test]
fn tank_busters_without_mitigation() {
    let mut f = mplus(true, 1200.0);
    f.players[0].class = "Warrior".into();
    f.players[0].spec = "Protection".into();
    f.tank_hits = vec![
        tank_hit("Brutalize", 10.0, &[]),
        tank_hit("Brutalize", 30.0, &[]),
        tank_hit("Head Bash", 50.0, &[]),
        tank_hit("Head Bash", 70.0, &["Ignore Pain"]),
    ];
    let s = run(f, &Config::default());
    let t = get(&s, "Tank", "Tank busters").unwrap();
    assert_eq!(t.severity, Severity::High);
    assert_eq!(
        t.message,
        "3 of 4 tank busters hit with no active mitigation or defensive up: Brutalize ×2, Head Bash ×1. Have Shield Block (physical only) or Ignore Pain up before they land."
    );
    // One bare hit in ten is noise.
    let mut ok = mplus(true, 1200.0);
    ok.tank_hits = (0..10)
        .map(|i| {
            tank_hit(
                "Brutalize",
                i as f64,
                if i == 0 { &[] } else { &["Ignore Pain"] },
            )
        })
        .collect();
    assert!(get(&run(ok, &Config::default()), "Tank", "Tank busters").is_none());
}

fn mplus_with_bosses(bosses: Vec<BossMarker>) -> FightReport {
    let mut f = mplus(true, 1800.0);
    if let FightKind::MythicPlus { bosses: b, .. } = &mut f.kind {
        *b = bosses;
    }
    f
}

fn boss(name: &str, at: f64, ready: bool, used: bool) -> BossMarker {
    BossMarker {
        name: name.into(),
        encounter_id: 1,
        start_s: at,
        end_s: at + 90.0,
        success: true,
        lust_ready: ready,
        lust_used: used,
        forces_pct: None,
    }
}

#[test]
fn lust_on_bosses_idle_trash_and_first_pull() {
    let mut f = mplus_with_bosses(vec![
        boss("First", 300.0, true, true),
        boss("Second", 900.0, true, false),
        boss("Third", 1500.0, false, false),
    ]);
    let p = |i: usize, spikes: u32, ready: f64, used: bool| PullReport {
        lust_used: used,
        lust_ready_s: ready,
        spikes,
        ..pull(i, 0.0, 1.0, 0)
    };
    f.pulls = vec![
        p(1, 1, 0.0, true),
        // Lust back after 10 min: 100 + 90 s ready on spiking trash, then a
        // quiet pull that doesn't count.
        p(2, 2, 100.0, false),
        p(3, 0, 300.0, false),
        p(4, 1, 90.0, false),
        p(5, 1, 50.0, true),
    ];
    let s = run(f, &Config::default());
    let lust: Vec<(&str, Severity)> = s
        .iter()
        .filter(|x| x.category == "Lust")
        .map(|x| (x.message.as_str(), x.severity))
        .collect();
    assert_eq!(
        lust,
        [
            (
                "Lust was available and not used on: Second at 15:00.",
                Severity::Medium
            ),
            (
                "Lust sat unused through dangerous trash: 3:10 of combat over pulls 2–4. Using it on a pack that's spiking the group saves time and deaths.",
                Severity::Medium
            ),
            (
                "Lust went out on the first pull. That's a valid choice for a fast start; it means any boss in the next 10 minutes goes without it.",
                Severity::Low
            ),
        ]
    );
    let quiet = mplus_with_bosses(vec![boss("First", 300.0, false, false)]);
    assert!(
        run(quiet, &Config::default())
            .iter()
            .all(|x| x.category != "Lust")
    );
}

// ----------------------------------------------------------------- dispels

fn debuff(player: &str, applied: f64, ended: f64, outcome: DebuffOutcome) -> DebuffRecord {
    DebuffRecord {
        spell_id: 1,
        name: "Cryo Surge".into(),
        player: player.into(),
        source: "Mob".into(),
        applied_s: applied,
        ended_s: ended,
        pull: Some(1),
        priority: true,
        types: vec![crate::classes::DispelType::Magic],
        outcome,
        ready_dispellers: vec![],
    }
}

fn rdisp(player: &str, in_range: Option<bool>) -> ReadyDispeller {
    ReadyDispeller {
        player: player.into(),
        ability: "Purify".into(),
        cd_s: 8.0,
        distance_yd: in_range.map(|r| if r { 18.0 } else { 55.0 }),
        in_range,
    }
}

fn dispelled(by: &str, latency_s: f64) -> DebuffOutcome {
    DebuffOutcome::Dispelled {
        by: by.into(),
        ability: "Purify".into(),
        latency_s,
    }
}

#[test]
fn undispelled_priority_debuffs_with_a_dispeller_ready() {
    let mut f = mplus(true, 1200.0);
    f.deaths = vec![died_with("Charlie", 300.0, 1, vec![])];
    let with = |mut d: DebuffRecord, r: Vec<ReadyDispeller>| {
        d.ready_dispellers = r;
        d
    };
    f.debuffs = vec![
        debuff("Alpha", 10.0, 11.5, dispelled("Healer", 1.5)),
        with(
            debuff("Bravo", 242.0, 252.0, DebuffOutcome::Expired),
            vec![rdisp("Healer", Some(true))],
        ),
        with(
            debuff("Charlie", 295.0, 300.0, DebuffOutcome::Died),
            vec![rdisp("Healer", Some(true))],
        ),
        // Not named: position unknown, out of range, nobody ready, not
        // priority, still up at the end, up under 2 s, or only a group
        // cooldown ready.
        with(
            debuff("Alpha", 800.0, 801.5, DebuffOutcome::Expired),
            vec![rdisp("Healer", Some(true))],
        ),
        with(
            debuff("Alpha", 900.0, 910.0, DebuffOutcome::Expired),
            vec![ReadyDispeller {
                ability: "Revival".into(),
                cd_s: 180.0,
                ..rdisp("Healer", Some(true))
            }],
        ),
        with(
            debuff("Alpha", 400.0, 410.0, DebuffOutcome::Expired),
            vec![rdisp("Healer", None)],
        ),
        with(
            debuff("Alpha", 500.0, 510.0, DebuffOutcome::Expired),
            vec![rdisp("Healer", Some(false))],
        ),
        debuff("Alpha", 600.0, 610.0, DebuffOutcome::Expired),
        DebuffRecord {
            priority: false,
            ..with(
                debuff("Alpha", 700.0, 710.0, DebuffOutcome::Expired),
                vec![rdisp("Healer", Some(true))],
            )
        },
        with(
            debuff("Alpha", 1190.0, 1200.0, DebuffOutcome::Lasted),
            vec![rdisp("Healer", Some(true))],
        ),
    ];
    let s = run(f, &Config::default());
    let d = get(&s, "group", "Dispels").unwrap();
    assert_eq!(d.severity, Severity::High);
    assert_eq!(
        d.message,
        "2 priority debuff(s) went undispelled while someone could have dispelled them, worst first. \
         Cryo Surge on Charlie when they died at 5:00 (pull 1); ready: Healer (Purify, 18 yd). \
         Cryo Surge on Bravo expired at 4:12 after 10 s (pull 1); ready: Healer (Purify, 18 yd). \
         1 of 9 priority debuffs were dispelled this run."
    );
}

#[test]
fn no_dispel_advice_for_late_wipe_deaths_or_outside_mplus() {
    let mut f = mplus(true, 1200.0);
    // A wiping pull: the third death isn't judged.
    f.deaths = vec![
        died_with("Tank", 300.0, 2, vec![]),
        died_with("Healer", 301.0, 2, vec![]),
        died_with("Alpha", 302.0, 2, vec![]),
        died_with("Bravo", 303.0, 2, vec![]),
    ];
    let mut d = debuff("Alpha", 295.0, 302.0, DebuffOutcome::Died);
    d.ready_dispellers = vec![rdisp("Healer", Some(true))];
    f.debuffs = vec![d.clone()];
    assert!(get(&run(f, &Config::default()), "group", "Dispels").is_none());

    let mut r = raid(false, 300.0, Some(40.0));
    d.outcome = DebuffOutcome::Expired;
    r.debuffs = vec![d];
    assert!(get(&run(r, &Config::default()), "group", "Dispels").is_none());
}

#[test]
fn slow_priority_dispels() {
    let mut f = mplus(true, 1200.0);
    for p in f.players.iter_mut() {
        match p.name.as_str() {
            "Healer" => {
                p.priority_dispels = 3;
                p.dispel_latency_s = Some(6.5);
            }
            // Too few samples, or fast enough.
            "Alpha" => {
                p.priority_dispels = 2;
                p.dispel_latency_s = Some(12.0);
            }
            "Bravo" => {
                p.priority_dispels = 5;
                p.dispel_latency_s = Some(4.0);
            }
            _ => {}
        }
    }
    f.debuffs = vec![
        debuff("Alpha", 60.0, 66.0, dispelled("Healer", 6.0)),
        debuff("Bravo", 120.0, 129.5, dispelled("Healer", 9.5)),
        debuff("Charlie", 180.0, 186.5, dispelled("Healer", 6.5)),
    ];
    let s = run(f, &Config::default());
    let h = get(&s, "Healer", "Dispel speed").unwrap();
    assert_eq!(h.severity, Severity::Low);
    assert_eq!(
        h.message,
        "Priority debuffs took a median 6.5 s to dispel (3 dispels). Slowest: Cryo Surge on Bravo at 2:00, after 9.5 s. Dispel these as soon as they land: a focus or mouseover macro helps."
    );
    assert!(get(&s, "Alpha", "Dispel speed").is_none());
    assert!(get(&s, "Bravo", "Dispel speed").is_none());
}

#[test]
fn death_penalty_comes_from_the_key() {
    let mut f = mplus(false, 1200.0);
    f.death_penalty_s = Some(5.0);
    f.deaths = vec![
        death("Alpha", 10.0, "Slam", 2),
        DeathReport {
            priority_debuffs: vec!["Cryo Surge".into()],
            ..death("Bravo", 20.0, "Slam", 2)
        },
    ];
    let s = run(f.clone(), &Config::default());
    assert!(
        get(&s, "group", "Deaths")
            .unwrap()
            .message
            .contains("At 5s per death that's roughly 0:10 added")
    );
    assert!(
        get(&s, "Bravo", "Death")
            .unwrap()
            .message
            .ends_with("Priority dispels up when you died: Cryo Surge at 0:20.")
    );
    assert!(
        !get(&s, "Alpha", "Death")
            .unwrap()
            .message
            .contains("Priority dispels")
    );
    assert!(
        get(&s, "group", "Timer")
            .unwrap()
            .evidence
            .starts_with("Biggest levers, in order: deaths")
    );

    f.death_penalty_s = Some(0.0);
    let s = run(f, &Config::default());
    let d = &get(&s, "group", "Deaths").unwrap().message;
    assert!(d.contains("Deaths cost no time in this key"), "{d}");
    assert!(!d.contains("added to the timer"));
    assert_eq!(
        get(&s, "group", "Timer").unwrap().evidence,
        "Deaths cost no time in this key. Biggest levers, in order: downtime, then damage output."
    );
}

#[test]
fn timer_advice_follows_upgrades_not_the_completion_flag() {
    let with = |success: bool, upgrades: Option<u8>| {
        let mut f = mplus(success, 1200.0);
        if let FightKind::MythicPlus {
            timer_s,
            upgrades: u,
            time_s,
            ..
        } = &mut f.kind
        {
            *timer_s = Some(1100.0);
            *u = upgrades;
            *time_s = 1172.0;
        }
        run(f, &Config::default())
    };
    // Completed but over the timer: depleted, with the margin.
    assert_eq!(
        get(&with(true, Some(0)), "group", "Timer").unwrap().message,
        "Key depleted: 19:32 against a 18:20 timer, 1:12 over."
    );
    assert!(get(&with(true, Some(1)), "group", "Timer").is_none());
    assert_eq!(
        get(&with(false, None), "group", "Timer").unwrap().message,
        "Key abandoned after 19:32."
    );
}

// ------------------------------------------------------------ pacing (M5)

use crate::reference::{RefBoss, RefSegment, Stat, TopRuns};

fn st(median: f64) -> Stat {
    Stat {
        median,
        min: median - 10.0,
        max: median + 10.0,
    }
}

fn segment(index: usize, boss: Option<&str>, start: f64, end: f64, pulls: Vec<usize>) -> Segment {
    Segment {
        index,
        boss: boss.map(Into::into),
        start_s: start,
        end_s: end,
        pulls,
        ..Default::default()
    }
}

/// Two bosses at 400 s and 1000 s; segment 2 has a 450 s trash pull with
/// 30% forces. 108% forces at the end over 900 s of trash combat.
fn route_fight() -> FightReport {
    let mut f = mplus(true, 1100.0);
    if let FightKind::MythicPlus { bosses, .. } = &mut f.kind {
        *bosses = vec![
            BossMarker {
                forces_pct: Some(40.0),
                ..boss("Kystia", 310.0, false, false)
            },
            BossMarker {
                forces_pct: Some(60.0),
                ..boss("Xathuux", 910.0, false, false)
            },
        ];
        bosses[0].encounter_id = 1;
        bosses[1].encounter_id = 2;
    }
    f.forces_pct = Some(108.0);
    f.downtime_s = 150.0;
    f.pulls = vec![
        PullReport {
            forces_pct: Some(40.0),
            ..pull(1, 10.0, 300.0, 0)
        },
        PullReport {
            boss: Some("Kystia".into()),
            ..pull(2, 310.0, 400.0, 0)
        },
        PullReport {
            forces_pct: Some(30.0),
            ..pull(3, 420.0, 870.0, 2)
        },
        PullReport {
            boss: Some("Xathuux".into()),
            ..pull(4, 910.0, 1000.0, 0)
        },
        PullReport {
            forces_pct: Some(38.0),
            ..pull(5, 1010.0, 1090.0, 0)
        },
    ];
    f.segments = vec![
        Segment {
            combat_s: 380.0,
            boss_s: 90.0,
            downtime_s: 20.0,
            ..segment(1, Some("Kystia"), 0.0, 400.0, vec![1, 2])
        },
        Segment {
            combat_s: 540.0,
            boss_s: 90.0,
            downtime_s: 60.0,
            deaths: 2,
            death_penalty_s: 30.0,
            ..segment(2, Some("Xathuux"), 400.0, 1000.0, vec![3, 4])
        },
        Segment {
            combat_s: 80.0,
            downtime_s: 20.0,
            ..segment(3, None, 1000.0, 1100.0, vec![5])
        },
    ];
    f
}

fn top_runs() -> TopRuns {
    TopRuns {
        runs: vec!["a:1".into(), "b:1".into(), "c:1".into()],
        level: 14,
        time_s: st(900.0),
        forces_pct: Some(st(102.0)),
        bosses: vec![
            RefBoss {
                name: "Kystia".into(),
                encounter_id: 1,
                kill_s: st(380.0),
                forces_pct: Some(st(42.0)),
            },
            RefBoss {
                name: "Xathuux".into(),
                encounter_id: 2,
                kill_s: st(800.0),
                forces_pct: Some(st(76.0)),
            },
        ],
        segments: vec![
            RefSegment {
                index: 1,
                boss: Some("Kystia".into()),
                duration_s: st(380.0),
                combat_s: st(340.0),
                downtime_s: st(40.0),
                forces_pct: Some(st(42.0)),
            },
            RefSegment {
                index: 2,
                boss: Some("Xathuux".into()),
                duration_s: st(420.0),
                combat_s: st(380.0),
                downtime_s: st(40.0),
                forces_pct: Some(st(34.0)),
            },
        ],
    }
}

#[test]
fn route_and_segments_against_top_runs() {
    let mut f = route_fight();
    f.reference = Some(top_runs());
    let s = run(f, &Config::default());
    // Kystia is 2% behind (under the 5% bar); Xathuux 16% behind.
    let r = get(&s, "group", "Route").unwrap();
    assert!(
        r.message.starts_with(
            "Behind the top runs on enemy forces at: Xathuux 60% (top runs 76%, 66–86%)."
        ),
        "{}",
        r.message
    );
    assert!(!r.message.contains("Kystia"));

    // Already at 100%: not behind, even if the top runs over-pulled.
    let mut f = route_fight();
    if let FightKind::MythicPlus { bosses, .. } = &mut f.kind {
        bosses[1].forces_pct = Some(101.0);
    }
    let mut t = top_runs();
    t.bosses[1].forces_pct = Some(st(112.0));
    f.reference = Some(t);
    assert!(get(&run(f, &Config::default()), "group", "Route").is_none());
    // Only segment 2 is slow: 600 s against 420 s. Segment 1 is 20 s slower.
    let seg = get(&s, "group", "Segments").unwrap();
    assert_eq!(seg.severity, Severity::High);
    assert_eq!(
        seg.message,
        "Slowest stretches against the top runs, worst first. Kystia → Xathuux: 10:00 against 7:00 (+3:00). \
         Combat 9:00 vs 6:20, downtime 1:00 vs 0:40, 2 death(s) (0:30 penalty). \
         Longest pulls: pull 3 7:30 (30% forces, 4.0%/min), pull 4 1:30 (Xathuux)."
    );
    // The overshoot is judged against 100%, not the top runs.
    assert!(get(&s, "group", "Overshoot").is_some());
    // The downtime advice is left to the segments.
    assert!(get(&s, "group", "Pacing").is_none());
}

#[test]
fn overshoot_and_pull_pace_without_top_runs() {
    let s = run(route_fight(), &Config::default());
    // 108% over 820 s of trash combat (1000 - 180 boss): 8% extra at 7.9%/min.
    let o = get(&s, "group", "Overshoot").unwrap();
    assert_eq!(
        o.message,
        "Killed 108% enemy forces, 8% more than needed: about 1:00 of trash at this key's pace (7.9% a minute). Skip a pack, or pull a smaller one instead."
    );
    assert_eq!(o.severity, Severity::Medium);
    // Pull 3: 4.0%/min against a median of 8.3: under half.
    let p = get(&s, "group", "Pull pace").unwrap();
    assert!(
        p.message.contains("pull 3 at 7:00 (7:30, 4.0%/min)"),
        "{}",
        p.message
    );
    assert!(get(&s, "group", "Segments").is_none());

    // Within 5% over: nothing.
    let mut f = route_fight();
    f.forces_pct = Some(104.0);
    assert!(get(&run(f, &Config::default()), "group", "Overshoot").is_none());
}

#[test]
fn timed_keys_get_their_margin() {
    let pack = crate::season::SeasonPack::parse(
        r#"
        [season]
        key = "t"
        name = "T"
        [[dungeon]]
        name = "Spire"
        keystone_id = 1
        map_id = 2000
        timer_s = [1800, 1440, 1080]
        "#,
    )
    .unwrap();
    let cfg = Config {
        knowledge: crate::season::Knowledge::build(Some(pack), &[]),
        ..Default::default()
    };
    let with = |time: f64, u: u8| {
        let mut f = mplus(true, time);
        if let FightKind::MythicPlus { upgrades, .. } = &mut f.kind {
            *upgrades = Some(u);
        }
        get(&run(f, &cfg), "group", "Timer").map(|s| s.message.clone())
    };
    assert_eq!(
        with(1500.0, 1).unwrap(),
        "+1 with 5:00 to spare; 1:00 short of +2."
    );
    assert_eq!(with(1000.0, 3).unwrap(), "+3 with 1:20 to spare.");
}
