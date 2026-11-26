use super::*;

const GOOD: &str = r#"
[season]
key = "test-s1"
name = "Test Season"
blizzard_season_id = 1

[[dungeon]]
name = "Voidscar Arena"
keystone_id = 585
map_id = 2923
journal_id = 1313
timer_s = [1800, 1440, 1080]
bosses = [
  { name = "Taz'Rah", journal_id = 2791, encounter_id = 3285 },
  { name = "Atroxus", journal_id = 2792 },
]

[[raid]]
name = "The Venomous Abyss"
map_id = 3004
bosses = [{ name = "Vashnik the Malignant", encounter_id = 3455 }]

[[spell]]
ids = [1222100, 1222103]
name = "Nether Dash"
map_id = 2923
boss = "Taz'Rah"
sources = ["journal", "log"]
tags = ["avoidable"]
note = "Step out of the line"

[[spell]]
ids = [500]
name = "Shadowbolt Volley"
map_id = 2923
suggested = ["priority_kick"]

[[spell]]
ids = [600]
name = "Affix Thing"
tags = ["affix", "dangerous_cast"]
"#;

fn with(extra: &str) -> Vec<String> {
    let pack: SeasonPack = toml::from_str(&format!("{GOOD}\n{extra}")).unwrap();
    pack.problems()
}

#[test]
fn good_pack_parses() {
    let p = SeasonPack::parse(GOOD).unwrap();
    assert_eq!(p.dungeons[0].bosses[1].encounter_id, 0);
    assert_eq!(p.dungeon_by_map(2923).unwrap().keystone_id, 585);
    assert!(p.dungeon_by_map(1).is_none());
    assert_eq!(p.spells[0].tags, [Tag::Avoidable]);
    assert_eq!(p.spells[0].sources, [Source::Journal, Source::Log]);
    assert_eq!(
        p.summary(),
        "Test Season (test-s1): 1 dungeons, 1 raids, 3 bosses (2 with encounter IDs), 3 spells. \
         Tagged: avoidable 1, dangerous_cast 1, affix 1. Awaiting review: 1."
    );
}

#[test]
fn every_embedded_pack_is_valid() {
    for (key, text) in EMBEDDED {
        let p = SeasonPack::parse(text).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_eq!(
            &p.season.key, key,
            "embedded key must match the pack's [season] key"
        );
    }
    SeasonPack::embedded();
}

#[test]
fn round_trips_through_toml() {
    // `season sync` rewrites packs, so serialising must not lose anything.
    let p = SeasonPack::parse(GOOD).unwrap();
    let again = SeasonPack::parse(&toml::to_string(&p).unwrap()).unwrap();
    assert_eq!(again.spells, p.spells);
    assert_eq!(again.dungeons[0].bosses, p.dungeons[0].bosses);
    assert_eq!(again.dungeons[0].timer_s, p.dungeons[0].timer_s);
    assert_eq!(again.season.blizzard_season_id, Some(1));
}

#[test]
fn duplicate_spell_ids_are_rejected() {
    let p = with("[[spell]]\nids = [1222103]\nname = \"Copy\"\n");
    assert_eq!(
        p,
        ["spell id 1222103 appears in both \"Nether Dash\" and \"Copy\""]
    );
}

#[test]
fn spells_must_point_at_known_instances_and_bosses() {
    let p = with(
        r#"
[[spell]]
ids = [1]
map_id = 9999
[[spell]]
ids = [2]
name = "Wrong Boss"
map_id = 2923
boss = "Nobody"
[[spell]]
ids = [3]
name = "No Map"
boss = "Taz'Rah"
"#,
    );
    assert_eq!(
        p,
        [
            "spell [1]: map_id 9999 is not a dungeon or raid in this pack",
            "spell \"Wrong Boss\": boss \"Nobody\" is not in \"Voidscar Arena\"",
            "spell \"No Map\": a boss needs a map_id",
        ]
    );
}

#[test]
fn instance_and_encounter_ids_must_be_unique() {
    let p = with(
        r#"
[[dungeon]]
name = "Clone"
keystone_id = 585
map_id = 2923
bosses = [{ name = "Vashnik Again", encounter_id = 3455 }]
"#,
    );
    assert!(
        p.iter()
            .any(|x| x.contains("keystone_id 585 is 0 or duplicated")),
        "{p:?}"
    );
    assert!(
        p.iter()
            .any(|x| x.contains("map_id 2923 is used by more than one instance")),
        "{p:?}"
    );
    assert!(p.iter().any(|x| x.contains("encounter_id 3455")), "{p:?}");
}

#[test]
fn misc_problems() {
    let p = with(
        r#"
[[spell]]
ids = []
name = "Empty"
[[spell]]
ids = [7]
name = "Both"
tags = ["avoidable"]
suggested = ["avoidable"]
[[spell]]
ids = [8]
name = "Excluded"
exclude = true
"#,
    );
    assert_eq!(
        p,
        [
            "spell \"Empty\" has no ids",
            "spell \"Both\": Avoidable is both tagged and suggested",
            "spell \"Excluded\": `exclude` only belongs in a user override",
        ]
    );
}

#[test]
fn parse_reports_every_problem_and_rejects_typos() {
    let e = SeasonPack::parse(&format!(
        "{GOOD}\n[[spell]]\nids = [0]\n[[spell]]\nids = []\n"
    ))
    .unwrap_err();
    let msg = e.to_string();
    assert!(
        msg.contains("spell id 0") && msg.contains("has no ids"),
        "{msg}"
    );

    // Unknown fields and tags fail loudly instead of being ignored.
    assert!(SeasonPack::parse(&GOOD.replace("journal_id = 1313", "jounral_id = 1313")).is_err());
    assert!(
        SeasonPack::parse(&GOOD.replace("tags = [\"avoidable\"]", "tags = [\"avoid\"]")).is_err()
    );
}

#[test]
fn knowledge_indexes_every_id_of_a_mechanic() {
    let k = Knowledge::build(Some(SeasonPack::parse(GOOD).unwrap()), &[]);
    assert_eq!(k.pack_key.as_deref(), Some("test-s1"));
    for id in [1222100, 1222103] {
        assert!(k.has_tag(id, Tag::Avoidable));
        assert_eq!(k.get(id).unwrap().note, "Step out of the line");
    }
    assert!(
        !k.has_tag(500, Tag::PriorityKick),
        "suggested is not tagged"
    );
    assert!(k.has_tag(600, Tag::Affix));
}
