use super::*;
use crate::config::{Config, SpellNote};
use crate::testutil::*;

fn note(id: u32, name: &str) -> SpellNote {
    SpellNote {
        spell_id: id,
        name: name.into(),
        note: String::new(),
    }
}

fn find<'a>(f: &'a FightReport, who: &Unit) -> &'a PlayerReport {
    f.players
        .iter()
        .find(|p| p.name == who.name)
        .unwrap_or_else(|| panic!("{} not in report", who.name))
}

/// Raid pull with every player hitting the boss once a second.
fn raid_pull(log: &mut Log, party: &Party, boss: &Unit, from: i64, to: i64) {
    let mut t = from;
    while t < to {
        for p in party.all() {
            log.damage(t, p, boss, 100, "Strike", 1000);
        }
        t += 1000;
    }
}

// ------------------------------------------------------------ segmentation

#[test]
fn raid_kill_is_one_fight_with_spec_roles() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.encounter_start(0, 3001, "Boss", 15, 5);
    raid_pull(&mut log, &party, &boss, 1000, 60_000);
    log.encounter_end(60_000, 3001, "Boss", 15, true);

    let fights = analyze(&log, Config::default());
    assert_eq!(fights.len(), 1);
    let f = &fights[0];
    assert_eq!(f.title, "Heroic Boss — Kill");
    assert_eq!(f.duration_s, 60.0);
    assert!(f.complete);
    assert!(matches!(
        f.kind,
        FightKind::Raid {
            success: true,
            boss_hp_pct: Some(0.0),
            ..
        }
    ));
    let roles: Vec<(Role, bool)> = f
        .players
        .iter()
        .map(|p| (p.role, p.role_inferred))
        .collect();
    assert_eq!(
        roles,
        vec![
            (Role::Tank, false),
            (Role::Healer, false),
            (Role::Dps, false),
            (Role::Dps, false),
            (Role::Dps, false)
        ]
    );
    assert_eq!(find(f, &party.tank).spec, "Blood");
}

#[test]
fn player_order_is_deterministic_on_ties() {
    // Equal damage: DPS must come out sorted by name every time, not in HashMap order.
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.encounter_start(0, 3001, "Boss", 15, 5);
    raid_pull(&mut log, &party, &boss, 1000, 10_000);
    log.encounter_end(10_000, 3001, "Boss", 15, true);
    for _ in 0..5 {
        let f = &analyze(&log, Config::default())[0];
        let names: Vec<&str> = f.players.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Tank-Realm",
                "Healer-Realm",
                "Alpha-Realm",
                "Bravo-Realm",
                "Charlie-Realm"
            ]
        );
    }
}

#[test]
fn raid_wipe_tracks_boss_hp() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 16, 20);
    raid_pull(&mut log, &party, &boss, 1000, 9000);
    log.damage_hp(
        9000,
        &party.dps[0],
        &boss,
        100,
        "Strike",
        1000,
        Some((400, 1000)),
    );
    log.encounter_end(10_000, 3001, "Boss", 16, false);

    let f = &analyze(&log, Config::default())[0];
    assert_eq!(f.title, "Mythic Boss — Wipe (40.0%)");
}

#[test]
fn fights_under_five_seconds_are_dropped() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 15, 5);
    raid_pull(&mut log, &party, &boss, 0, 4000);
    log.encounter_end(4000, 3001, "Boss", 15, false);
    assert!(analyze(&log, Config::default()).is_empty());
}

#[test]
fn mplus_run_splits_pulls_and_measures_downtime() {
    let party = Party::new();
    let mob = enemy("Trash", 1);
    let boss = enemy("Big Boss", 2);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.key_start(0, "Spire", 2000, 10);
    for t in [1000, 2000, 3000] {
        log.damage(t, &party.dps[0], &mob, 100, "Strike", 1000);
    }
    log.encounter_start(19_000, 3010, "Big Boss", 8, 5);
    for t in [20_000, 21_000] {
        log.damage(t, &party.dps[0], &boss, 100, "Strike", 1000);
    }
    log.encounter_end(25_000, 3010, "Big Boss", 8, true);
    log.key_end(30_000, 2000, true, 30_000);

    let fights = analyze(&log, Config::default());
    assert_eq!(fights.len(), 1);
    let f = &fights[0];
    assert_eq!(f.title, "+10 Spire — Timed");
    assert!(f.complete);
    assert_eq!(f.pulls.len(), 2);
    assert_eq!((f.pulls[0].start_s, f.pulls[0].end_s), (1.0, 3.0));
    assert_eq!(f.pulls[0].boss, None);
    assert_eq!(f.pulls[1].start_s, 20.0);
    assert_eq!(f.pulls[1].boss.as_deref(), Some("Big Boss"));
    // Combat = (1.0..3.0 + GCD) + (20.0..21.0 + GCD) = 3.3 + 2.3 = 5.6s of 30s.
    assert!((f.downtime_s - 24.4).abs() < 1e-9, "{}", f.downtime_s);
    let FightKind::MythicPlus {
        success,
        time_s,
        bosses,
        ..
    } = &f.kind
    else {
        panic!("not M+")
    };
    assert!(*success);
    assert_eq!(*time_s, 30.0);
    assert_eq!(bosses.len(), 1);
    assert_eq!((bosses[0].start_s, bosses[0].end_s), (19.0, 25.0));
}

#[test]
fn leaving_the_dungeon_ends_an_abandoned_key() {
    let party = Party::new();
    let mob = enemy("Trash", 1);
    let mut log = Log::default();
    log.key_start(0, "Spire", 2000, 10);
    for t in (1000..=8000).step_by(1000) {
        log.damage(t, &party.dps[0], &mob, 100, "Strike", 1000);
    }
    log.zone(60_000, 1);
    // Open-world activity afterwards must not be swallowed by the run.
    log.damage(61_000, &party.dps[0], &mob, 100, "Strike", 1000);

    let fights = analyze(&log, Config::default());
    assert_eq!(fights.len(), 1);
    let f = &fights[0];
    assert!(!f.complete);
    assert_eq!(f.duration_s, 8.0);
    assert_eq!(f.title, "+10 Spire — Depleted / abandoned");
    assert_eq!(find(f, &party.dps[0]).damage_done, 8000);
}

// --------------------------------------------------------------- casts

#[test]
fn enemy_casts_count_kicks_and_successes() {
    let party = Party::new();
    let caster = enemy("Caster", 1);
    let other = enemy("Other Caster", 2);
    let mut log = Log::default();
    log.key_start(0, "Spire", 2000, 10);
    log.cast_start(1000, &caster, 500, "Bolt")
        .interrupt(1500, &party.dps[0], &caster, 500, "Bolt")
        .cast_start(3000, &caster, 500, "Bolt")
        .cast(4500, &caster, 500, "Bolt")
        // Instant version without a cast bar: couldn't have been kicked.
        .cast(6000, &caster, 500, "Bolt")
        // Same name, different ID (another mob): merged by name.
        .cast_start(6500, &other, 501, "Bolt")
        .interrupt(7000, &party.dps[1], &other, 501, "Bolt")
        // Never kicked and not configured: not reported.
        .cast_start(7500, &caster, 600, "Volley")
        .cast(9000, &caster, 600, "Volley");
    log.key_end(10_000, 2000, false, 10_000);

    let f = &analyze(&log, Config::default())[0];
    assert_eq!(f.enemy_casts.len(), 1);
    let bolt = &f.enemy_casts[0];
    assert_eq!(
        (
            bolt.spell_id,
            bolt.started,
            bolt.succeeded,
            bolt.interrupted,
            bolt.priority
        ),
        (500, 3, 1, 2, false)
    );
    assert_eq!(find(f, &party.dps[0]).interrupts, 1);
    assert_eq!(find(f, &party.dps[1]).interrupts, 1);

    // Listing Volley as a priority kick reports it even though nobody kicked it.
    let cfg = Config {
        priority_interrupt: vec![note(600, "Volley")],
        ..Config::default()
    };
    let f = &analyze(&log, cfg)[0];
    let volley = f.enemy_casts.iter().find(|c| c.name == "Volley").unwrap();
    assert_eq!(
        (volley.succeeded, volley.interrupted, volley.priority),
        (1, 0, true)
    );
    // Most casts that went through first.
    assert_eq!(f.enemy_casts[0].name, "Bolt");
}

#[test]
fn activity_counts_gcds_while_alive() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let (bravo, charlie) = (&party.dps[1], &party.dps[2]);
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 15, 5);
    for t in (0..6500).step_by(1300) {
        log.cast(t, bravo, 100, "Strike")
            .cast(t, charlie, 100, "Strike");
    }
    // Bravo is dead for the second half, so the idle time doesn't count against them.
    log.death(6500, bravo);
    log.damage(12_000, charlie, &boss, 100, "Strike", 1);
    log.encounter_end(13_000, 3001, "Boss", 15, true);

    let f = &analyze(&log, Config::default())[0];
    assert!((find(f, bravo).activity_pct - 1.0).abs() < 1e-9);
    assert!((find(f, charlie).activity_pct - 0.5).abs() < 1e-9);
    assert_eq!(find(f, charlie).casts, 5);
}

// -------------------------------------------------------------- deaths

#[test]
fn death_report_has_recap_and_consumables() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let victim = &party.dps[0];
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 15, 5);
    log.damage(1000, &boss, victim, 1, "A", 100)
        .cast(5000, victim, 6262, "Healthstone")
        // Hits stay within pull_gap_ms of each other, so this is all one pull.
        .damage(5500, &boss, victim, 2, "B", 200)
        .damage(8000, &boss, victim, 3, "C", 300)
        .damage_hp(9000, &boss, victim, 4, "D", 400, Some((0, 1000)))
        .death(10_000, victim)
        // Duplicate death event (e.g. Spirit of Redemption) is ignored.
        .death(10_500, victim);
    log.encounter_end(20_000, 3001, "Boss", 15, true);

    let f = &analyze(&log, Config::default())[0];
    assert_eq!(f.deaths.len(), 1);
    let d = &f.deaths[0];
    assert_eq!(d.player, victim.name);
    assert_eq!(d.at_s, 10.0);
    assert_eq!(d.killing_blow, "D (Boss)");
    assert_eq!(d.killing_spell_id, 4);
    assert_eq!(d.damage_last_5s, 900);
    assert!(d.healthstone_used_before);
    assert!(!d.potion_used_before);
    assert_eq!(d.pull, Some(1));
    assert!(!d.wipe_cascade);
    let spells: Vec<&str> = d.recap.iter().map(|r| r.spell.as_str()).collect();
    assert_eq!(spells, ["A", "B", "C", "D"]);
    assert_eq!(d.recap[3].hp_pct_after, Some(0.0));
    assert_eq!(find(f, victim).deaths, 1);
}

#[test]
fn raid_wipe_marks_cascade_deaths() {
    // 8 players: once max(8/4, 3) = 3 are dead, later deaths are cascade noise.
    let boss = enemy("Boss", 1);
    let raid: Vec<Unit> = (0..8).map(|i| player(&format!("P{i}"))).collect();
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 16, 20);
    for p in &raid {
        log.damage(1000, p, &boss, 100, "Strike", 1000);
    }
    for (i, p) in raid.iter().take(5).enumerate() {
        log.death(10_000 + i as i64 * 1000, p);
    }
    log.encounter_end(30_000, 3001, "Boss", 16, false);

    let f = &analyze(&log, Config::default())[0];
    let cascade: Vec<bool> = f.deaths.iter().map(|d| d.wipe_cascade).collect();
    assert_eq!(cascade, [false, false, false, true, true]);
}

// ------------------------------------------------------- attribution etc.

#[test]
fn pets_are_credited_to_their_owner() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let wolf = pet("Wolf", 9);
    let owner = &party.dps[0];
    let mut log = Log::default();
    log.summon(0, owner, &wolf);
    log.encounter_start(0, 3001, "Boss", 15, 5);
    // The owner has to show up under their own name once; pet events alone
    // only carry the owner's GUID.
    log.cast(500, owner, 100, "Strike")
        .damage(1000, &wolf, &boss, 100, "Bite", 5000)
        // Primal Rage from the pet counts as lust; a second one within 60s doesn't.
        .cast(2000, &wolf, 264667, "Primal Rage")
        .cast(3000, &wolf, 264667, "Primal Rage");
    log.encounter_end(10_000, 3001, "Boss", 15, true);

    let f = &analyze(&log, Config::default())[0];
    assert_eq!(f.players.len(), 1);
    assert_eq!(find(f, owner).damage_done, 5000);
    assert_eq!(f.lust_at_s, [2.0]);
}

#[test]
fn deferred_damage_and_absorbs() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 15, 5);
    log.damage(1000, &boss, &party.tank, 1, "Melee", 500)
        // Stagger tick: already counted when the original hit was absorbed.
        .damage(2000, &boss, &party.tank, 124255, "Stagger", 1000)
        .push(
            3000,
            Some(&boss),
            Some(&party.tank),
            Kind::Absorbed {
                absorber: party.healer.clone(),
                spell: spell(17, "Power Word: Shield"),
                amount: 3000,
            },
        )
        // Stagger's own "absorb" only delays damage: no healing credit.
        .push(
            4000,
            Some(&boss),
            Some(&party.tank),
            Kind::Absorbed {
                absorber: party.tank.clone(),
                spell: spell(115069, "Stagger"),
                amount: 9999,
            },
        )
        .heal(5000, &party.healer, &party.tank, 1000, 400);
    log.encounter_end(10_000, 3001, "Boss", 15, true);

    let f = &analyze(&log, Config::default())[0];
    assert_eq!(find(f, &party.tank).damage_taken, 500);
    assert_eq!(find(f, &party.tank).healing_done, 0);
    let h = find(f, &party.healer);
    assert_eq!((h.healing_done, h.absorbs_done), (3600, 3000));
    assert!((h.overheal_pct - 0.1).abs() < 1e-9); // 400 of 4000
}

#[test]
fn roles_are_inferred_without_combatant_info() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    log.encounter_start(0, 3001, "Boss", 15, 5);
    log.damage(1000, &boss, &party.tank, 1, "Melee", 5000)
        .damage(1000, &boss, &party.dps[0], 1, "Melee", 100)
        .heal(2000, &party.healer, &party.tank, 4000, 0)
        .damage(3000, &party.dps[0], &boss, 100, "Strike", 9000);
    log.encounter_end(10_000, 3001, "Boss", 15, true);

    let f = &analyze(&log, Config::default())[0];
    for (u, role) in [
        (&party.tank, Role::Tank),
        (&party.healer, Role::Healer),
        (&party.dps[0], Role::Dps),
    ] {
        let p = find(f, u);
        assert_eq!((p.role, p.role_inferred), (role, true), "{}", p.name);
    }
}

// ----------------------------------------------------- avoidable damage

fn swirl_log(party: &Party) -> Log {
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.encounter_start(0, 3001, "Boss", 15, 5);
    // Swirl hits only Alpha (3x); Pulse hits every non-tank once.
    for t in [1000, 2000, 3000] {
        log.damage(t, &boss, &party.dps[0], 900, "Swirl", 100_000);
    }
    for u in [&party.healer, &party.dps[0], &party.dps[1], &party.dps[2]] {
        log.damage(4000, &boss, u, 901, "Pulse", 50_000);
    }
    log.encounter_end(10_000, 3001, "Boss", 15, true);
    log
}

#[test]
fn uneven_hits_are_flagged_possibly_avoidable() {
    let party = Party::new();
    let f = &analyze(&swirl_log(&party), Config::default())[0];
    let swirl = f.enemy_damage.iter().find(|s| s.spell_id == 900).unwrap();
    let pulse = f.enemy_damage.iter().find(|s| s.spell_id == 901).unwrap();
    assert!(swirl.likely_avoidable && !swirl.configured_avoidable);
    assert_eq!(swirl.non_tank_hit_frac, 0.25);
    assert!(!pulse.likely_avoidable);
    assert_eq!(find(f, &party.dps[0]).likely_avoidable_taken, 300_000);
    assert_eq!(find(f, &party.dps[1]).likely_avoidable_taken, 0);
    // Biggest total first.
    assert_eq!(f.enemy_damage[0].spell_id, 900);
}

#[test]
fn configured_avoidable_overrides_the_heuristic() {
    let party = Party::new();
    let cfg = Config {
        avoidable: vec![note(900, "Swirl")],
        ..Config::default()
    };
    let f = &analyze(&swirl_log(&party), cfg)[0];
    let swirl = f.enemy_damage.iter().find(|s| s.spell_id == 900).unwrap();
    assert!(swirl.configured_avoidable && !swirl.likely_avoidable);
    assert_eq!(find(f, &party.dps[0]).avoidable_taken, 300_000);
    assert_eq!(find(f, &party.dps[0]).likely_avoidable_taken, 0);
}

// ---------------------------------------------------------- progression

#[test]
fn progression_groups_pulls_of_the_same_boss() {
    let party = Party::new();
    let boss = enemy("Boss", 1);
    let mut log = Log::default();
    let mut t = 0;
    for (success, first_killer) in [(false, "Cleave"), (false, "Cleave"), (true, "Fire")] {
        log.encounter_start(t, 3001, "Boss", 16, 20);
        raid_pull(&mut log, &party, &boss, t + 1000, t + 5000);
        log.damage(t + 5000, &boss, &party.dps[0], 7, first_killer, 1)
            .death(t + 5500, &party.dps[0]);
        log.encounter_end(t + 10_000, 3001, "Boss", 16, success);
        t += 100_000;
    }
    // A different boss with a single pull gets no progression entry.
    log.encounter_start(t, 3002, "Other", 16, 20);
    raid_pull(&mut log, &party, &boss, t + 1000, t + 9000);
    log.encounter_end(t + 10_000, 3002, "Other", 16, false);

    let fights = analyze(&log, Config::default());
    assert_eq!(fights.len(), 4);
    let prog = progression(&fights);
    assert_eq!(prog.len(), 1);
    let p = &prog[0];
    assert_eq!((p.attempts, p.kills), (3, 1));
    assert_eq!(p.best_hp_pct, Some(0.0));
    assert_eq!(p.first_death_causes[0], ("Cleave".to_string(), 2));
    assert_eq!(p.frequent_deaths[0], (party.dps[0].name.clone(), 3));
}

// ------------------------------------------------------- spell evidence

#[test]
fn spell_evidence_for_season_suggest() {
    let party = Party::new();
    let caster = enemy("Caster", 1);
    let boss = enemy("Big Boss", 2);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.key_start(0, "Spire", 2000, 10);
    // Trash: a kicked cast, a completed one, and an instant one.
    log.cast_start(1000, &caster, 500, "Bolt")
        .interrupt(1500, &party.dps[0], &caster, 500, "Bolt")
        .cast_start(2000, &caster, 500, "Bolt")
        .cast(3500, &caster, 500, "Bolt")
        .cast(4000, &caster, 500, "Bolt")
        // A debuff, dispelled once out of two.
        .debuff(4500, &caster, &party.dps[1], 600, "Curse")
        .debuff(4600, &caster, &party.dps[2], 600, "Curse")
        .dispel(5000, &party.healer, &party.dps[1], 600, "Curse")
        // A buff on an enemy is not evidence.
        .push(
            5100,
            Some(&caster),
            Some(&caster),
            Kind::AuraApplied {
                spell: spell(601, "Enrage"),
                is_buff: true,
            },
        );
    // Boss fight: tank-only hits.
    log.encounter_start(10_000, 3010, "Big Boss", 8, 5);
    for t in [11_000, 12_000, 13_000] {
        log.damage(t, &boss, &party.tank, 700, "Crush", 50_000);
    }
    log.damage(13_500, &boss, &party.dps[0], 700, "Crush", 10_000);
    // Melee: ID 1 in raw logs, 0 from Warcraft Logs. Neither is evidence.
    log.cast(14_000, &boss, 0, "Melee")
        .cast(14_100, &boss, 1, "Melee");
    log.encounter_end(20_000, 3010, "Big Boss", 8, true);
    log.key_end(30_000, 2000, true, 30_000);

    let f = &analyze(&log, Config::default())[0];
    let FightKind::MythicPlus { bosses, .. } = &f.kind else {
        panic!()
    };
    assert_eq!(bosses[0].encounter_id, 3010);

    let ids: Vec<u32> = f.spell_evidence.iter().map(|e| e.spell_id).collect();
    assert_eq!(
        ids,
        [500, 600, 700],
        "sorted; enemy buffs and melee excluded"
    );
    let get = |id| f.spell_evidence.iter().find(|e| e.spell_id == id).unwrap();

    let bolt = get(500);
    assert_eq!(bolt.boss, None, "trash");
    assert_eq!(
        (
            bolt.casts_started,
            bolt.casts_succeeded,
            bolt.interrupted,
            bolt.instant_casts
        ),
        (2, 1, 1, 1)
    );
    let curse = get(600);
    assert_eq!((curse.applied, curse.dispelled), (2, 1));
    let crush = get(700);
    assert_eq!(crush.boss.as_deref(), Some("Big Boss"));
    assert_eq!(
        (crush.hits, crush.damage, crush.tank_damage),
        (4, 160_000, 150_000)
    );
    assert_eq!(crush.players_hit, 2);
}
