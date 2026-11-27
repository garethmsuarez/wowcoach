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

// --------------------------------------------------------- cast timeline

/// Party specs: Blood DK tank (Mind Freeze 15s, 15 yd), Discipline healer
/// (no interrupt), three Arcane mages (Counterspell 24s, 40 yd).
fn outcomes_run() -> (Party, Vec<Unit>, Log) {
    let party = Party::new();
    let mobs: Vec<Unit> = (1..=6).map(|n| enemy("Caster", n)).collect();
    let (alpha, bravo, charlie) = (&party.dps[0], &party.dps[1], &party.dps[2]);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.key_start(0, "Spire", 2000, 10);
    log.damage(900, alpha, &mobs[0], 100, "Strike", 1);
    // Kicked by Alpha 0.8s into the bar.
    log.cast_start(1000, &mobs[0], 500, "Bolt").kick_with(
        1800,
        alpha,
        &mobs[0],
        2139,
        "Counterspell",
        500,
        "Bolt",
    );
    // Stunned by the tank's Asphyxiate, then the mob starts another cast.
    log.cast_start(5000, &mobs[1], 501, "Hex")
        .debuff(5500, &party.tank, &mobs[1], 221562, "Asphyxiate")
        .cast_start(8000, &mobs[1], 502, "Later");
    // Killed mid-cast.
    log.cast_start(9000, &mobs[2], 503, "Nova")
        .death(9500, &mobs[2]);
    // Ends silently with nothing to explain it.
    log.cast_start(10_000, &mobs[3], 504, "Shriek")
        .cast_start(13_000, &mobs[3], 505, "Again");
    // Charlie dies; the tank uses Mind Freeze just before the next cast ends.
    log.death(14_000, charlie);
    log.cast(29_000, &party.tank, 47528, "Mind Freeze");
    // Completes at 30s. Alpha's Counterspell (24s) is back since 25.8s, 10 yd
    // away; Bravo never used his, but stands 90 yd away.
    log.at(29_500, alpha, 0.0, 0.0)
        .at(29_500, bravo, 100.0, 0.0)
        .at(29_500, &mobs[4], 10.0, 0.0)
        .cast_start(28_000, &mobs[4], 500, "Bolt")
        .cast(30_000, &mobs[4], 500, "Bolt");
    log.key_end(40_000, 2000, true, 40_000);
    (party, mobs, log)
}

#[test]
fn cast_bars_record_how_they_ended() {
    let (_, _, log) = outcomes_run();
    let cfg = Config {
        priority_interrupt: vec![note(500, "Bolt")],
        ..Config::default()
    };
    let f = &analyze(&log, cfg)[0];
    let by_spell = |id: u32| f.casts.iter().filter(move |c| c.spell_id == id);
    let kicked = by_spell(500).next().unwrap();
    assert_eq!(
        kicked.outcome,
        CastOutcome::Kicked {
            by: "Alpha-Realm".into(),
            spell: "Counterspell".into(),
            latency_s: 0.8
        }
    );
    assert!(kicked.priority);
    assert_eq!(kicked.caster_npc, 1);
    assert_eq!(
        by_spell(501).next().unwrap().outcome,
        CastOutcome::Stopped {
            by: "Tank-Realm".into(),
            ability: "Asphyxiate".into(),
            cc: "stun".into()
        }
    );
    assert_eq!(
        by_spell(503).next().unwrap().outcome,
        CastOutcome::CasterDied
    );
    assert_eq!(
        by_spell(504).next().unwrap().outcome,
        CastOutcome::Cancelled
    );
    // Bars still open when the key ends aren't recorded.
    assert!(by_spell(502).next().is_none() && by_spell(505).next().is_none());

    let done = by_spell(500).nth(1).unwrap();
    assert_eq!(done.outcome, CastOutcome::Completed);
    assert_eq!((done.start_s, done.end_s), (28.0, 30.0));
    let ready: Vec<(&str, &str, Option<f32>, Option<bool>)> = done
        .ready_kickers
        .iter()
        .map(|k| {
            (
                k.player.as_str(),
                k.ability.as_str(),
                k.distance_yd,
                k.in_range,
            )
        })
        .collect();
    // Charlie is dead, the tank's Mind Freeze is on cooldown, the healer has none.
    assert_eq!(
        ready,
        [
            ("Alpha-Realm", "Counterspell", Some(10.0), Some(true)),
            ("Bravo-Realm", "Counterspell", Some(90.0), Some(false)),
        ]
    );
    let p = &f.pulls[0];
    assert_eq!(
        (
            p.casts_kicked,
            p.casts_stopped,
            p.casters_killed,
            p.priority_completed
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn stale_positions_give_no_distance() {
    let (party, mobs, mut log) = outcomes_run();
    // Same run, but Alpha's last position is 5s old when the cast completes.
    log.events.retain(|e| {
        !(e.ts == 29_500 && e.src.as_ref().is_some_and(|s| s.guid == party.dps[0].guid))
    });
    log.at(25_000, &party.dps[0], 0.0, 0.0);
    log.events.sort_by_key(|e| e.ts);
    let f = &analyze(&log, Config::default())[0];
    let done = f
        .casts
        .iter()
        .find(|c| c.outcome == CastOutcome::Completed && c.caster_npc == npc_id(&mobs[4]))
        .unwrap();
    let alpha = done
        .ready_kickers
        .iter()
        .find(|k| k.player == "Alpha-Realm")
        .unwrap();
    assert_eq!((alpha.distance_yd, alpha.in_range), (None, None));
}

fn npc_id(u: &Unit) -> u32 {
    super::casts::npc_id(&u.guid)
}

#[test]
fn observed_faster_reuse_counts_as_the_cooldown() {
    // A warrior who reuses Pummel every 8s (talents) is ready again sooner
    // than the table's 15s.
    let war = player("War");
    let mob = enemy("Caster", 1);
    let mut log = Log::default();
    log.combatant(0, &war, 73).key_start(0, "Spire", 2000, 10);
    log.damage(100, &war, &mob, 100, "Strike", 1);
    log.cast(1000, &war, 6552, "Pummel")
        .cast(9000, &war, 6552, "Pummel");
    log.cast_start(15_000, &mob, 500, "Bolt")
        .cast(17_500, &mob, 500, "Bolt");
    log.key_end(20_000, 2000, true, 20_000);
    let f = &analyze(&log, Config::default())[0];
    let done = &f.casts[0];
    assert_eq!(done.outcome, CastOutcome::Completed);
    assert_eq!(
        done.ready_kickers.len(),
        1,
        "9s + 8s observed cd = ready at 17s"
    );
    assert_eq!(done.ready_kickers[0].ability, "Pummel");
}

#[test]
fn ready_stoppers_for_stoppable_casts() {
    use crate::season::{Knowledge, SpellEntry, Tag};
    // A Windwalker monk who used Leg Sweep earlier has it ready again when a
    // stoppable Bolt goes off; the mage never used a stop, and the Feral
    // druid's Rake only stuns from stealth, so it's never offered.
    let monk = player("Monk");
    let mage = player("Mage");
    let cat = player("Cat");
    let mob = enemy("Caster", 1);
    let mut log = Log::default();
    log.combatant(0, &monk, 269)
        .combatant(0, &mage, 62)
        .combatant(0, &cat, 103)
        .key_start(0, "Spire", 2000, 10);
    log.damage(100, &monk, &mob, 100, "Strike", 1);
    log.cast(1000, &monk, 119381, "Leg Sweep");
    log.cast(1500, &cat, 1822, "Rake");
    log.at(69_500, &monk, 0.0, 0.0)
        .at(69_500, &mage, 30.0, 0.0)
        .at(69_500, &cat, 2.0, 0.0)
        .at(69_500, &mob, 3.0, 0.0);
    log.cast_start(68_000, &mob, 500, "Bolt")
        .cast(70_000, &mob, 500, "Bolt");
    log.cast_start(71_000, &mob, 501, "Filler")
        .cast(73_000, &mob, 501, "Filler");
    log.key_end(80_000, 2000, true, 80_000);
    let cfg = Config {
        knowledge: Knowledge::build(
            None,
            &[SpellEntry {
                ids: vec![500],
                tags: vec![Tag::Stoppable],
                ..Default::default()
            }],
        ),
        ..Config::default()
    };
    let f = &analyze(&log, cfg)[0];
    let bolt = f.casts.iter().find(|c| c.spell_id == 500).unwrap();
    let stops: Vec<(&str, &str, Option<bool>)> = bolt
        .ready_stoppers
        .iter()
        .map(|k| (k.player.as_str(), k.ability.as_str(), k.in_range))
        .collect();
    assert_eq!(stops, [("Monk-Realm", "Leg Sweep", Some(true))]);
    let filler = f.casts.iter().find(|c| c.spell_id == 501).unwrap();
    assert!(filler.ready_stoppers.is_empty(), "not tagged stoppable");
}

#[test]
fn evidence_counts_stops() {
    let (_, _, log) = outcomes_run();
    let f = &analyze(&log, Config::default())[0];
    let hex = f.spell_evidence.iter().find(|e| e.spell_id == 501).unwrap();
    assert_eq!(hex.stopped, 1, "Asphyxiate stopped the Hex");
}

// ------------------------------------------------------------ defensives

fn tagged(tags: &[(u32, crate::season::Tag)]) -> Config {
    use crate::season::{Knowledge, SpellEntry};
    let entries: Vec<SpellEntry> = tags
        .iter()
        .map(|(id, t)| SpellEntry {
            ids: vec![*id],
            tags: vec![*t],
            ..Default::default()
        })
        .collect();
    Config {
        knowledge: Knowledge::build(None, &entries),
        ..Config::default()
    }
}

/// Keeps one M+ pull going from `from` to `to` (a hit every second).
fn keep_pull(log: &mut Log, who: &Unit, mob: &Unit, from: i64, to: i64) {
    for t in (from..=to).step_by(1000) {
        log.damage(t, who, mob, 100, "Strike", 1);
    }
}

#[test]
fn defensive_buff_logged_before_its_cast_is_one_use() {
    // Blur's buff (212800) is logged 50 ms before its cast (198589), as raw
    // logs do. Die by the Sword's buff is its cast ID.
    let dh = player("Dh");
    let war = player("War");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &dh, 577)
        .combatant(0, &war, 71)
        .key_start(0, "Spire", 2000, 10);
    keep_pull(&mut log, &dh, &mob, 100, 80_000);
    log.buff(1950, &dh, &dh, 212800, "Blur")
        .cast(2000, &dh, 198589, "Blur")
        .fade(12_000, &dh, &dh, 212800, true);
    log.buff(60_000, &war, &war, 118038, "Die by the Sword")
        .cast(60_000, &war, 118038, "Die by the Sword");
    log.death(62_000, &war).death(70_000, &dh);
    log.key_end(90_000, 2000, true, 90_000);
    let f = &analyze(&log, Config::default())[0];
    let uses: Vec<(&str, &str, &str, f64)> = f
        .defensive_uses
        .iter()
        .map(|u| {
            (
                u.player.as_str(),
                u.ability.as_str(),
                u.scope.as_str(),
                u.at_s,
            )
        })
        .collect();
    assert_eq!(
        uses,
        [
            ("Dh-Realm", "Blur", "personal", 1.95),
            ("War-Realm", "Die by the Sword", "personal", 60.0)
        ]
    );
    let died = |n: &str| f.deaths.iter().find(|d| d.player == n).unwrap();
    // The warrior died with Die by the Sword up, so it wasn't ready.
    let w = died("War-Realm");
    assert_eq!(w.defensives_active, ["Die by the Sword"]);
    assert!(w.defensives_ready.is_empty());
    // Blur (60 s) was back by the DH's death and had faded.
    let d = died("Dh-Realm");
    assert!(d.defensives_active.is_empty());
    assert_eq!(
        d.defensives_ready,
        [ReadyDefensive {
            player: "Dh-Realm".into(),
            ability: "Blur".into(),
            last_used_s: 1.95
        }]
    );
}

#[test]
fn observed_reuse_shortens_a_defensive_cooldown() {
    // Fast reused Blur after 40 s (talents), so by 82.5 s it's ready again;
    // Slow used it once at 41 s and the table's 60 s still applies.
    let fast = player("Fast");
    let slow = player("Slow");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &fast, 577)
        .combatant(0, &slow, 577)
        .key_start(0, "Spire", 2000, 10);
    keep_pull(&mut log, &fast, &mob, 100, 85_000);
    log.cast(1000, &fast, 198589, "Blur")
        .cast(41_000, &fast, 198589, "Blur")
        .cast(41_000, &slow, 198589, "Blur");
    log.death(82_500, &fast).death(82_500, &slow);
    log.key_end(90_000, 2000, true, 90_000);
    let f = &analyze(&log, Config::default())[0];
    let ready = |n: &str| {
        !f.deaths
            .iter()
            .find(|d| d.player == n)
            .unwrap()
            .defensives_ready
            .is_empty()
    };
    assert!(ready("Fast-Realm"));
    assert!(!ready("Slow-Realm"));
}

#[test]
fn spikes_record_group_defensives_used_and_ready() {
    // Two players with 1000 HP each: losing 1000 within 5 s is a spike.
    // Rallying Cry answers the first; by the second (after its 180 s
    // cooldown) it's ready but unused, and the healer dies.
    let war = player("War");
    let heal = player("Heal");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &war, 71)
        .combatant(0, &heal, 264)
        .key_start(0, "Spire", 2000, 10);
    log.hit(500, &mob, &heal, 900, 1, 1, 1000);
    log.cast(1000, &war, 97462, "Rallying Cry");
    for t in [2000, 3000, 4000] {
        log.hit(t, &mob, &war, 900, 1, 400, 1000);
    }
    // A small hit that's no spike.
    log.hit(60_000, &mob, &war, 900, 1, 300, 1000);
    for t in [200_000, 201_000, 202_000] {
        log.hit(t, &mob, &heal, 901, 32, 400, 1000);
    }
    log.death(203_000, &heal);
    log.key_end(210_000, 2000, true, 210_000);
    let f = &analyze(&log, Config::default())[0];
    assert_eq!(f.spikes.len(), 2, "{:?}", f.spikes);
    let (a, b) = (&f.spikes[0], &f.spikes[1]);
    assert_eq!(
        a.group_used,
        [GroupDefensive {
            player: "War-Realm".into(),
            ability: "Rallying Cry".into(),
            at_s: 1.0
        }]
    );
    assert!(a.group_ready.is_empty() && a.deaths.is_empty());
    assert!((a.peak_frac - 0.6005).abs() < 1e-9, "{}", a.peak_frac);
    assert_eq!(a.top[0].spell_id, 900);
    assert!(b.group_used.is_empty());
    assert_eq!(
        b.group_ready,
        [ReadyDefensive {
            player: "War-Realm".into(),
            ability: "Rallying Cry".into(),
            last_used_s: 1.0
        }]
    );
    assert_eq!(b.deaths, ["Heal-Realm"]);
    assert_eq!((b.start_s, b.end_s), (200.0, 202.0));
    let pulls: Vec<u32> = f.pulls.iter().map(|p| p.spikes).collect();
    assert_eq!(pulls.iter().sum::<u32>(), 2);
}

#[test]
fn tank_busters_check_mitigation_against_the_school() {
    use crate::season::Tag;
    // A Prot warrior: Shield Block (physical only) up 1-5 s, Ignore Pain up
    // since before the key until 8 s (only its fade is logged).
    let tank = player("Tank");
    let mob = enemy("Boss", 1);
    let mut log = Log::default();
    log.combatant(0, &tank, 73).key_start(0, "Spire", 2000, 10);
    log.buff(1000, &tank, &tank, 132404, "Shield Block");
    log.hit(1500, &mob, &tank, 700, 1, 500, 1000) // physical
        .hit(2000, &mob, &tank, 701, 32, 500, 1000); // shadow
    log.fade(5000, &tank, &tank, 132404, true);
    log.hit(6000, &mob, &tank, 700, 1, 500, 1000);
    log.fade(8000, &tank, &tank, 190456, true);
    log.hit(9000, &mob, &tank, 700, 1, 500, 1000);
    log.hit(9500, &mob, &tank, 702, 1, 500, 1000); // not a tank buster
    log.key_end(20_000, 2000, true, 20_000);
    let cfg = tagged(&[(700, Tag::TankBuster), (701, Tag::TankBuster)]);
    let f = &analyze(&log, cfg)[0];
    let got: Vec<(f64, u32, Vec<&str>)> = f
        .tank_hits
        .iter()
        .map(|h| {
            (
                h.at_s,
                h.school,
                h.mitigation.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (1.5, 1, vec!["Ignore Pain", "Shield Block"]),
            (2.0, 32, vec!["Ignore Pain"]),
            (6.0, 1, vec!["Ignore Pain"]),
            (9.0, 1, vec![]),
        ]
    );
    assert_eq!(f.pulls[0].tank_hits, 4);
    assert_eq!(f.pulls[0].tank_hits_unmitigated, 1);
}

#[test]
fn sated_gates_lust() {
    // Pull 1: lust and Sated at 1.5 s, half a second before the first hit.
    // Pull 2 (100 s): everyone Sated. Sated
    // fades at 600 s, so pull 3 (610 s) has lust ready again.
    let sham = player("Sham");
    let war = player("War");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &sham, 262)
        .combatant(0, &war, 71)
        .key_start(0, "Spire", 2000, 10);
    // Lust goes out just before the first hit, as groups do.
    log.cast(1500, &sham, 2825, "Bloodlust");
    for p in [&sham, &war] {
        log.debuff(1500, &sham, p, 57724, "Sated");
    }
    keep_pull(&mut log, &war, &mob, 2000, 10_000);
    keep_pull(&mut log, &war, &mob, 100_000, 110_000);
    for p in [&sham, &war] {
        log.fade(600_000, &sham, p, 57724, false);
    }
    keep_pull(&mut log, &war, &mob, 610_000, 620_000);
    log.key_end(630_000, 2000, true, 630_000);
    let f = &analyze(&log, Config::default())[0];
    let got: Vec<(bool, f64, bool)> = f
        .pulls
        .iter()
        .map(|p| (p.lust_ready_at_start, p.lust_ready_s, p.lust_used))
        .collect();
    assert_eq!(
        got,
        [(true, 0.0, true), (false, 0.0, false), (true, 10.0, false)]
    );
}

#[test]
fn a_lust_cast_without_a_logged_sated_still_counts() {
    // No Sated debuff in the log: the cast itself makes lust unavailable
    // for 10 minutes.
    let sham = player("Sham");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &sham, 262).key_start(0, "Spire", 2000, 10);
    log.cast(1500, &sham, 2825, "Bloodlust");
    keep_pull(&mut log, &sham, &mob, 2000, 10_000);
    keep_pull(&mut log, &sham, &mob, 100_000, 110_000);
    // Sated (from 1.5 s) is over by this pull's lead-in at 605 s.
    keep_pull(&mut log, &sham, &mob, 615_000, 620_000);
    log.key_end(630_000, 2000, true, 630_000);
    let f = &analyze(&log, Config::default())[0];
    let got: Vec<bool> = f.pulls.iter().map(|p| p.lust_ready_at_start).collect();
    assert_eq!(got, [true, false, true]);
}

#[test]
fn no_lust_class_means_no_lust_available() {
    let war = player("War");
    let mob = enemy("Mob", 1);
    let mut log = Log::default();
    log.combatant(0, &war, 71).key_start(0, "Spire", 2000, 10);
    keep_pull(&mut log, &war, &mob, 1000, 10_000);
    log.key_end(20_000, 2000, true, 20_000);
    let f = &analyze(&log, Config::default())[0];
    assert!(!f.pulls[0].lust_ready_at_start);
}

#[test]
fn enemy_totems_are_tracked_and_named_in_spikes() {
    // A shaman drops a Magma Totem at 1 s; it hits both players (1000 HP
    // each) for 600, a spike, and is only killed at 13 s.
    let war = player("War");
    let heal = player("Heal");
    let shaman = enemy("Shaman", 1);
    let totem = enemy("Magma Totem", 2);
    let mut log = Log::default();
    log.combatant(0, &war, 71)
        .combatant(0, &heal, 264)
        .key_start(0, "Spire", 2000, 10);
    keep_pull(&mut log, &war, &shaman, 500, 14_000);
    log.summon(1000, &shaman, &totem);
    log.hit(3000, &totem, &war, 1246825, 4, 600, 1000)
        .hit(4000, &totem, &heal, 1246825, 4, 600, 1000);
    log.death(13_000, &totem);
    log.key_end(20_000, 2000, true, 20_000);
    let f = &analyze(&log, Config::default())[0];
    let s = &f.enemy_summons;
    assert_eq!(s.len(), 1);
    assert_eq!(
        (
            s[0].name.as_str(),
            s[0].summoner.as_str(),
            s[0].npc,
            s[0].killed
        ),
        ("Magma Totem", "Shaman", 2, true)
    );
    assert_eq!((s[0].start_s, s[0].end_s), (1.0, 13.0));
    assert_eq!(s[0].damage_to_group, 1200);
    let top = &f.spikes[0].top[0];
    assert_eq!(
        (
            top.source.as_str(),
            top.summoned_by.as_deref(),
            top.summon_age_s
        ),
        ("Magma Totem", Some("Shaman"), Some(2.0))
    );
}
