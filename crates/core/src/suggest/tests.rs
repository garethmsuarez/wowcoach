use super::*;
use crate::config::Config;
use crate::testutil::*;

const MAP: u32 = 2923;

fn pack() -> SeasonPack {
    SeasonPack::parse(
        r#"
        [season]
        key = "t"
        name = "T"
        [[dungeon]]
        name = "Voidscar Arena"
        keystone_id = 585
        map_id = 2923
        bosses = [
          { name = "Taz'Rah", journal_id = 2791 },
          { name = "Dazar, The First King", journal_id = 2000 },
        ]
        [[spell]]
        ids = [900]
        name = "Swirl"
        map_id = 2923
        tags = ["avoidable"]
        [[spell]]
        ids = [1222098]
        name = "Nether Dash"
        map_id = 2923
        boss = "Taz'Rah"
        sources = ["journal"]
        "#,
    )
    .unwrap()
}

/// A Voidscar key: a caster whose Bolt gets kicked `kicks` times out of 5,
/// a Curse dispelled 3 of 4 times, uneven Dark Swirl hits, a Nether Dash under a
/// new ID, and a Taz'Rah fight with tank-only Crush casts.
fn run(kicks: u32, map_id: u32) -> FightReport {
    let party = Party::new();
    let caster = enemy("Caster", 1);
    let boss = enemy("Taz'Rah", 2);
    let mut log = Log::default();
    party.log_specs(&mut log, 0);
    log.key_start(0, "Voidscar Arena", map_id, 12);
    let mut t = 1000;
    for i in 0..5 {
        log.cast_start(t, &caster, 500, "Bolt");
        if i < kicks {
            log.interrupt(t + 500, &party.dps[0], &caster, 500, "Bolt");
        } else {
            log.cast(t + 1500, &caster, 500, "Bolt");
        }
        t += 2000;
    }
    for (i, d) in [&party.dps[0], &party.dps[1], &party.dps[2], &party.healer]
        .iter()
        .enumerate()
    {
        log.debuff(t, &caster, d, 600, "Curse");
        if i < 3 {
            log.dispel(t + 300, &party.healer, d, 600, "Curse");
        }
        t += 500;
    }
    for _ in 0..3 {
        log.damage(t, &caster, &party.dps[0], 901, "Dark Swirl", 100_000);
        t += 500;
    }
    for d in [&party.healer, &party.dps[0], &party.dps[1], &party.dps[2]] {
        log.damage(t, &caster, d, 902, "Pulse", 20_000);
    }
    log.damage(t, &caster, &party.dps[1], 1222100, "Nether Dash", 5000)
        .damage(
            t + 100,
            &caster,
            &party.dps[2],
            1222100,
            "Nether Dash",
            5000,
        )
        .damage(
            t + 200,
            &caster,
            &party.dps[0],
            1222100,
            "Nether Dash",
            5000,
        );
    log.encounter_start(30_000, 3285, "Taz'Rah", 8, 5);
    for k in 0..3 {
        let ts = 31_000 + k * 3000;
        log.cast_start(ts, &boss, 700, "Crush")
            .cast(ts + 1500, &boss, 700, "Crush")
            .damage(ts + 1500, &boss, &party.tank, 700, "Crush", 400_000);
    }
    log.encounter_end(45_000, 3285, "Taz'Rah", 8, true);
    log.key_end(60_000, map_id, true, 60_000);
    analyze(&log, Config::default()).remove(0)
}

fn spell(p: &SeasonPack, id: u32) -> &SpellEntry {
    p.spells
        .iter()
        .find(|s| s.ids.contains(&id))
        .unwrap_or_else(|| panic!("no spell {id}"))
}

#[test]
fn learns_ids_encounters_and_proposals() {
    let mut p = pack();
    let mut c = Collected::default();
    for _ in 0..2 {
        c.add(&p, &run(4, MAP), Origin::Own);
    }
    let a = apply(&mut p, &c);
    assert_eq!(p.problems(), Vec::<String>::new());

    assert_eq!(
        p.dungeons[0].bosses[0].encounter_id, 3285,
        "filled from the boss marker"
    );
    assert_eq!(a.encounter_ids_filled, 1);

    let bolt = spell(&p, 500);
    assert_eq!(
        (bolt.boss.as_deref(), bolt.sources.as_slice()),
        (None, &[Source::Log][..])
    );
    assert_eq!(bolt.suggested, [Tag::PriorityKick]);
    assert!(bolt.tags.is_empty(), "tools never set tags");
    assert!(
        bolt.evidence.starts_with("yours (2 fights): kicked 8/10"),
        "{}",
        bolt.evidence
    );

    assert_eq!(spell(&p, 600).suggested, [Tag::DispelPriority]);
    let crush = spell(&p, 700);
    assert_eq!(crush.boss.as_deref(), Some("Taz'Rah"));
    assert_eq!(crush.suggested, [Tag::TankBuster]);
    assert!(
        spell(&p, 902).suggested.is_empty(),
        "even hits: nothing to suggest"
    );

    // The logged Nether Dash ID joins the journal entry of the same name.
    let dash = spell(&p, 1222100);
    assert_eq!(dash.ids, [1222098, 1222100]);
    assert_eq!(dash.sources, [Source::Journal, Source::Log]);
    assert_eq!(a.ids_added, 1);

    let names: Vec<&str> = a.proposals.iter().map(|x| x.2.as_str()).collect();
    assert!(
        names.contains(&"Bolt") && names.contains(&"Crush"),
        "{names:?}"
    );
    assert!(review(&c, &a).contains("Bolt (trash) -> priority_kick"));
}

#[test]
fn uneven_hits_across_fights_suggest_avoidable() {
    let mut p = pack();
    let mut c = Collected::default();
    for _ in 0..2 {
        c.add(&p, &run(0, MAP), Origin::Own);
    }
    apply(&mut p, &c);
    assert_eq!(spell(&p, 901).suggested, [Tag::Avoidable]);
    assert!(
        spell(&p, 500).suggested.is_empty(),
        "never kicked: no priority_kick"
    );
    // Once tagged, it's not suggested again.
    let i = p.spells.iter().position(|s| s.ids.contains(&901)).unwrap();
    p.spells[i].tags = vec![Tag::Avoidable];
    p.spells[i].suggested.clear();
    apply(&mut p, &c);
    assert!(spell(&p, 901).suggested.is_empty());
}

#[test]
fn top_keys_outweigh_our_own_kicking() {
    let mut p = pack();
    let mut c = Collected::default();
    c.add(&p, &run(0, MAP), Origin::Own);
    c.add(&p, &run(5, MAP), Origin::Top);
    apply(&mut p, &c);
    let bolt = spell(&p, 500);
    assert_eq!(bolt.suggested, [Tag::PriorityKick], "top groups kick 5/5");
    assert_eq!(bolt.sources, [Source::Log, Source::Wcl]);
    assert!(
        bolt.evidence
            .contains("yours (1 fight): kicked 0/5; top keys (1 fight): kicked 5/5"),
        "{}",
        bolt.evidence
    );
}

#[test]
fn dismissed_and_tagged_are_not_proposed_and_reruns_are_stable() {
    let mut p = pack();
    let mut c = Collected::default();
    for _ in 0..2 {
        c.add(&p, &run(4, MAP), Origin::Own);
    }
    apply(&mut p, &c);
    // Reviewer: Bolt promoted, Curse rejected.
    let i = p.spells.iter().position(|s| s.ids.contains(&500)).unwrap();
    p.spells[i].tags = std::mem::take(&mut p.spells[i].suggested);
    let j = p.spells.iter().position(|s| s.ids.contains(&600)).unwrap();
    p.spells[j].dismissed = std::mem::take(&mut p.spells[j].suggested);

    let a = apply(&mut p, &c);
    assert!(spell(&p, 500).suggested.is_empty());
    assert_eq!(spell(&p, 500).tags, [Tag::PriorityKick]);
    assert!(spell(&p, 600).suggested.is_empty());
    assert_eq!(a.spells_added, 0);
    assert!(a.proposals.is_empty());
    let before = crate::season::render(&p, "").unwrap();
    apply(&mut p, &c);
    assert_eq!(
        crate::season::render(&p, "").unwrap(),
        before,
        "re-run changes nothing"
    );
}

#[test]
fn unknown_instances_are_skipped_and_rare_spells_ignored() {
    let mut p = pack();
    let mut c = Collected::default();
    c.add(&p, &run(4, 9999), Origin::Own);
    assert_eq!(c.fights_skipped.len(), 1);
    assert!(c.spells.is_empty());
    assert!(apply(&mut p, &c).proposals.is_empty());

    // A spell seen fewer than MIN_EVENTS times isn't added.
    let mut one = run(4, MAP);
    one.spell_evidence.retain(|e| e.spell_id == 500);
    one.spell_evidence[0].casts_started = 1;
    one.spell_evidence[0].interrupted = 0;
    one.spell_evidence[0].instant_casts = 0;
    let mut c = Collected::default();
    c.add(&p, &one, Origin::Own);
    let before = p.spells.len();
    apply(&mut p, &c);
    assert_eq!(p.spells.len(), before);
}

#[test]
fn journal_and_log_boss_names_are_matched() {
    let p = pack();
    let bosses = &p.dungeons[0].bosses;
    assert_eq!(
        match_boss(bosses, "King Dazar", 0).unwrap().name,
        "Dazar, The First King"
    );
    assert_eq!(match_boss(bosses, "taz'rah", 0).unwrap().name, "Taz'Rah");
    assert!(match_boss(bosses, "Someone Else", 0).is_none());
    let two = [
        Boss {
            name: "Blade King".into(),
            ..Default::default()
        },
        Boss {
            name: "King of Fangs".into(),
            ..Default::default()
        },
    ];
    assert!(
        match_boss(&two, "King Dazar", 0).is_none(),
        "ambiguous word: no guess"
    );
}

#[test]
fn same_name_joins_an_existing_entry() {
    // A new logged ID with the name of a tagged pack spell joins that entry;
    // its tags stay the reviewer's and nothing is re-suggested.
    let mut p = pack();
    let mut f = run(0, MAP);
    for e in f.spell_evidence.iter_mut().filter(|e| e.spell_id == 901) {
        e.name = "Swirl".into();
    }
    let mut c = Collected::default();
    c.add(&p, &f, Origin::Own);
    c.add(&p, &f, Origin::Own);
    apply(&mut p, &c);
    let swirl = spell(&p, 901);
    assert_eq!(swirl.ids, [900, 901]);
    assert_eq!(swirl.tags, [Tag::Avoidable]);
    assert!(swirl.suggested.is_empty(), "already tagged");
}

#[test]
fn one_mechanic_with_several_ids_is_judged_once() {
    // Nether Dash under two logged IDs in the same fights: counted per fight,
    // not per ID, and reviewed as one entry.
    let mut p = pack();
    let mut c = Collected::default();
    for _ in 0..2 {
        let mut f = run(4, MAP);
        let mut twin = f
            .spell_evidence
            .iter()
            .find(|e| e.spell_id == 500)
            .unwrap()
            .clone();
        twin.spell_id = 501;
        f.spell_evidence.push(twin);
        c.add(&p, &f, Origin::Own);
    }
    let a = apply(&mut p, &c);
    let bolt = spell(&p, 501);
    assert_eq!(bolt.ids, [500, 501]);
    assert!(
        bolt.evidence.starts_with("yours (2 fights): kicked 16/20"),
        "{}",
        bolt.evidence
    );
    assert_eq!(a.proposals.iter().filter(|x| x.2 == "Bolt").count(), 1);
}

#[test]
fn a_single_fight_proposes_nothing() {
    let mut p = pack();
    let mut c = Collected::default();
    c.add(&p, &run(5, MAP), Origin::Own);
    let a = apply(&mut p, &c);
    assert!(a.proposals.is_empty());
    assert!(spell(&p, 500).suggested.is_empty());
    assert!(
        spell(&p, 500).evidence.contains("kicked 5/5"),
        "evidence is still recorded"
    );
}

#[test]
fn near_identical_boss_names_match() {
    let bosses = [
        Boss {
            name: "Lightwarden Ruia".into(),
            ..Default::default()
        },
        Boss {
            name: "Ziekket".into(),
            ..Default::default()
        },
    ];
    assert_eq!(match_boss(&bosses, "Ziekett", 0).unwrap().name, "Ziekket");
    assert!(match_boss(&bosses, "Zekk", 0).is_none());
}

#[test]
fn cached_evidence_gives_the_same_result() {
    // `suggest --wcl-top` caches FightEvidence as JSON instead of event streams.
    let f = run(4, MAP);
    let json = serde_json::to_string(&vec![FightEvidence::from(&f)]).unwrap();
    let back: Vec<FightEvidence> = serde_json::from_str(&json).unwrap();

    let (mut p1, mut p2) = (pack(), pack());
    let (mut c1, mut c2) = (Collected::default(), Collected::default());
    for _ in 0..2 {
        c1.add(&p1, &f, Origin::Top);
        c2.add_evidence(&p2, &back[0], Origin::Top);
    }
    apply(&mut p1, &c1);
    apply(&mut p2, &c2);
    assert_eq!(
        crate::season::render(&p1, "").unwrap(),
        crate::season::render(&p2, "").unwrap()
    );
}

#[test]
fn repeated_stops_suggest_stoppable() {
    let mut p = pack();
    let mut c = Collected::default();
    for n in [2, 1] {
        let mut f = run(4, MAP);
        f.spell_evidence
            .iter_mut()
            .find(|e| e.spell_id == 500)
            .unwrap()
            .stopped = n;
        c.add(&p, &f, Origin::Top);
    }
    apply(&mut p, &c);
    let bolt = spell(&p, 500);
    assert!(
        bolt.suggested.contains(&Tag::Stoppable),
        "{:?}",
        bolt.suggested
    );
    assert!(
        bolt.evidence.contains("stopped by CC 3"),
        "{}",
        bolt.evidence
    );
}
