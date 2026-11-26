use super::*;
use crate::model::Unit;
use crate::testutil::*;

fn table(extra: &str) -> Vec<String> {
    toml::from_str::<ClassData>(extra).unwrap().problems()
}

#[test]
fn embedded_table_is_valid_and_covers_every_spec() {
    let d = ClassData::embedded();
    // Only these healers lack a class interrupt.
    assert!(
        d.summary().ends_with(
            "Specs without a class interrupt: Discipline Priest, Holy Priest, Restoration Druid."
        ),
        "{}",
        d.summary()
    );
    for (id, _, spec, _) in crate::specs::SPECS {
        assert!(
            !d.for_spec(*id, AbilityKind::Defensive).is_empty(),
            "{spec} ({id}) has no defensive"
        );
    }
}

#[test]
fn lookups_respect_specs_and_racials() {
    let d = ClassData::embedded();
    let names = |spec, kind| -> Vec<String> {
        d.for_spec(spec, kind)
            .iter()
            .map(|a| a.name.clone())
            .collect()
    };
    // Guardian druid: Skull Bash, not Solar Beam; racials are listed too.
    let guardian = names(104, AbilityKind::Interrupt);
    assert!(guardian.contains(&"Skull Bash".into()) && !guardian.contains(&"Solar Beam".into()));
    assert!(guardian.contains(&"Arcane Torrent".into()));
    assert!(names(102, AbilityKind::Interrupt).contains(&"Solar Beam".into()));
    // Restoration druids dispel magic; other druids don't.
    let resto = d.for_spec(105, AbilityKind::Dispel);
    assert!(
        resto
            .iter()
            .any(|a| a.types.contains(&DispelType::Magic) && a.target == Some(Target::Friendly))
    );
    let feral = d.for_spec(103, AbilityKind::Dispel);
    assert!(
        !feral
            .iter()
            .any(|a| a.types.contains(&DispelType::Magic) && a.target == Some(Target::Friendly))
    );
    // The interrupt effect ID resolves to the same ability as the cast.
    assert_eq!(d.by_id(93985)[0].name, "Skull Bash");
    assert!(
        d.for_spec(1, AbilityKind::Interrupt).is_empty(),
        "unknown spec"
    );
}

#[test]
fn problems_are_specific() {
    let p = table(
        r#"
        [[ability]]
        ids = [1]
        name = "A"
        class = "Wizard"
        kind = "interrupt"
        cd_s = 15
        range_yd = 5
        [[ability]]
        ids = [2]
        name = "B"
        class = "Druid"
        specs = ["Frost"]
        kind = "stop"
        cd_s = 0
        [[ability]]
        ids = [3]
        name = "C"
        class = "Monk"
        kind = "dispel"
        types = ["magic"]
        cd_s = 8
        [[ability]]
        ids = [4]
        name = "D"
        class = "Monk"
        kind = "interrupt"
        scope = "personal"
        cd_s = 8
        range_yd = 5
        [[ability]]
        ids = [4]
        name = "E"
        class = "Monk"
        kind = "interrupt"
        cd_s = 8
        range_yd = 5
        "#,
    );
    let want = [
        "A (Wizard, Interrupt): unknown class",
        "B (Druid, Stop): \"Frost\" is not a Druid spec",
        "B (Druid, Stop): cd_s must be positive",
        "B (Druid, Stop): a stop needs `cc`",
        "C (Monk, Dispel): a dispel needs `types` and `target`",
        "D (Monk, Interrupt): has fields for another kind",
        "E (Monk, Interrupt): id 4 is also listed for \"D\" with the same kind",
    ];
    assert_eq!(p, want);
    assert!(
        toml::from_str::<ClassData>(
            "[[ability]]\nids=[1]\nname=\"x\"\nclass=\"Monk\"\nkind=\"kick\"\ncd_s=1\n"
        )
        .is_err()
    );
}

#[test]
fn audit_finds_missing_interrupts_fast_reuse_and_unseen() {
    let data = ClassData::parse(
        r#"
        [[ability]]
        ids = [6552]
        name = "Pummel"
        class = "Warrior"
        kind = "interrupt"
        cd_s = 15
        range_yd = 5
        [[ability]]
        ids = [97462]
        name = "Rallying Cry"
        class = "Warrior"
        kind = "defensive"
        scope = "group"
        cd_s = 180
        [[ability]]
        ids = [4987]
        name = "Cleanse"
        class = "Paladin"
        kind = "dispel"
        types = ["magic"]
        target = "friendly"
        cd_s = 8
        "#,
    )
    .unwrap();
    let (war, pal) = (player("War"), player("Pal"));
    let mob = enemy("Caster", 1);
    let mut log = Log::default();
    log.combatant(0, &war, 73).combatant(0, &pal, 65);
    // Pummel 4 times, 4s apart (listed cd 15): usually too fast. Plus an unlisted interrupt.
    log.cast(1000, &war, 6552, "Pummel")
        .cast(9000, &war, 6552, "Pummel")
        .cast(13_000, &war, 6552, "Pummel")
        .push(
            1000,
            Some(&war),
            Some(&mob),
            crate::model::Kind::Interrupt {
                spell: spell(6552, "Pummel"),
                extra: spell(1, "Bolt"),
            },
        )
        .cast(5000, &war, 6552, "Pummel")
        .push(
            6000,
            Some(&war),
            Some(&mob),
            crate::model::Kind::Interrupt {
                spell: spell(386071, "Disrupting Shout"),
                extra: spell(1, "Bolt"),
            },
        )
        .dispel(7000, &pal, &war, 99, "Curse")
        .cast(7000, &pal, 4987, "Cleanse");
    let mut a = Audit::default();
    for e in &log.events {
        a.feed(e);
    }
    let r = a.report(&data);
    assert_eq!(r.players, 2);
    // `dispel()` in testutil uses Purify (527), which isn't listed for paladins.
    assert_eq!(
        r.unlisted_dispels,
        ["Purify (527) dispelled 1x for Holy Paladin"]
    );
    assert_eq!(
        r.missing_interrupts,
        ["Disrupting Shout (386071) used 1x by Protection Warrior"]
    );
    assert_eq!(r.fast_reuse.len(), 1);
    assert_eq!(
        r.fast_reuse[0],
        "Pummel (Warrior): 3 of 3 reuses under 60% of the listed 15s, median 4.0s"
    );
    assert_eq!(r.unseen, ["Rallying Cry (Warrior: Protection)"]);
    assert!(
        r.to_string()
            .contains("ERROR interrupts used but not listed (1):")
    );
}

#[test]
fn companions_are_not_credited_to_players() {
    // A delve companion (Creature-) owned by a player interrupts: not the
    // player's ability. A class pet (Pet-) is.
    let data = ClassData::parse(
        "[[ability]]\nids = [19647]\nname = \"Spell Lock\"\nclass = \"Warlock\"\nkind = \"interrupt\"\ncd_s = 24\nrange_yd = 40\n",
    )
    .unwrap();
    let lock = player("Lock");
    let felhunter = pet("Felhunter", 7);
    let companion = Unit {
        guid: "Creature-0-1-1-1-248567-0000000001".into(),
        name: "Valeera Sanguinar".into(),
        flags: crate::model::TYPE_NPC | crate::model::REACTION_FRIENDLY,
    };
    let mob = enemy("Caster", 1);
    let mut log = Log::default();
    log.combatant(0, &lock, 265)
        .summon(0, &lock, &felhunter)
        .summon(0, &lock, &companion);
    for (src, id, name) in [
        (&felhunter, 19647, "Spell Lock"),
        (&companion, 1248172, "Dagger to the Throat"),
    ] {
        log.push(
            1000,
            Some(src),
            Some(&mob),
            crate::model::Kind::Interrupt {
                spell: spell(id, name),
                extra: spell(1, "Bolt"),
            },
        );
    }
    let mut a = Audit::default();
    for e in &log.events {
        a.feed(e);
    }
    let r = a.report(&data);
    assert!(
        r.missing_interrupts.is_empty(),
        "{:?}",
        r.missing_interrupts
    );
    assert_eq!(r.players, 1);
}

#[test]
fn ranges_and_stop_auras_are_validated() {
    let p = table(
        r#"
        [[ability]]
        ids = [1]
        name = "NoRange"
        class = "Monk"
        kind = "interrupt"
        cd_s = 15
        [[ability]]
        ids = [2]
        name = "Huge"
        class = "Monk"
        kind = "interrupt"
        cd_s = 15
        range_yd = 500
        [[ability]]
        ids = [3]
        name = "Wall"
        class = "Monk"
        kind = "defensive"
        scope = "personal"
        cd_s = 60
        range_yd = 5
        aura_ids = [9]
        "#,
    );
    assert_eq!(
        p,
        [
            "NoRange (Monk, Interrupt): an interrupt needs `range_yd`",
            "Huge (Monk, Interrupt): range_yd 500 is out of 1..100",
            "Wall (Monk, Defensive): range_yd is for interrupts and stops",
            "Wall (Monk, Defensive): aura_ids is for stops",
        ]
    );
    // Stops resolve by their aura, falling back to the cast IDs.
    let d = ClassData::embedded();
    assert!(
        d.stop_by_aura(119381)
            .is_some_and(|a| a.name == "Leg Sweep")
    );
}

fn at(log: &mut Log, ts: i64, u: &Unit, x: f32, y: f32) {
    // Any event with advanced info carries the unit's position.
    log.push(
        ts,
        Some(u),
        None,
        crate::model::Kind::CastSuccess {
            spell: spell(999_999, "Something"),
            info: Some(crate::model::AdvInfo {
                guid: u.guid.clone(),
                owner_guid: "0000000000000000".into(),
                hp: 1,
                max_hp: 1,
                pos: Some(crate::model::Pos { x, y, map: 1 }),
            }),
        },
    );
}

#[test]
fn audit_measures_kick_distance_and_discovers_stops() {
    let data = ClassData::parse(
        r#"
        [[ability]]
        ids = [6552]
        name = "Pummel"
        class = "Warrior"
        kind = "interrupt"
        cd_s = 15
        range_yd = 5
        [[ability]]
        ids = [46968]
        name = "Shockwave"
        class = "Warrior"
        kind = "stop"
        cc = "stun"
        cd_s = 40
        aura_ids = [132168]
        "#,
    )
    .unwrap();
    let war = player("War");
    let mob = enemy("Caster", 1);
    let mut log = Log::default();
    log.combatant(0, &war, 73).key_start(0, "Spire", 2000, 10);
    let kick = |log: &mut Log, ts| {
        log.push(
            ts,
            Some(&war),
            Some(&mob),
            crate::model::Kind::Interrupt {
                spell: spell(6552, "Pummel"),
                extra: spell(1, "Bolt"),
            },
        );
    };
    // Five Pummels from 20 yd away: further than a melee kick can reach.
    let mut t = 1000;
    for _ in 0..5 {
        at(&mut log, t, &war, 0.0, 0.0);
        at(&mut log, t, &mob, 20.0, 0.0);
        log.cast_start(t, &mob, 500, "Bolt");
        kick(&mut log, t + 500);
        t += 20_000;
    }
    // Five casts ended silently by Shockwave (listed), five by "Stomp" (not
    // listed), and many that completed with only a DoT on the caster.
    for (aura, name) in [(132168, "Shockwave"), (777, "Stomp")] {
        for _ in 0..5 {
            log.cast_start(t, &mob, 600, "Hex")
                .debuff(t + 200, &war, &mob, aura, name)
                .cast_start(t + 3000, &mob, 601, "Next");
            log.cast(t + 4500, &mob, 601, "Next");
            t += 10_000;
        }
    }
    // Realistic proportions: most casts complete (lift needs a baseline).
    for _ in 0..45 {
        log.cast_start(t, &mob, 600, "Hex")
            .debuff(t + 200, &war, &mob, 888, "Rend")
            .cast(t + 2000, &mob, 600, "Hex");
        t += 10_000;
    }
    log.key_end(t, 2000, true, t);
    let mut a = Audit::default();
    for e in &log.events {
        a.feed(e);
    }
    let r = a.report(&data);
    assert_eq!(
        r.kick_ranges,
        ["Pummel (6552): 5 kicks, median 20.0 yd, 90th percentile 20.0 yd; listed range 5 yd"]
    );
    assert_eq!((r.silent_casts, r.silent_explained), (10, 5));
    assert_eq!(r.stop_candidates.len(), 1, "{:?}", r.stop_candidates);
    assert!(
        r.stop_candidates[0].starts_with("Stomp (777): during 5 silent endings, 0 completed casts")
    );
}
