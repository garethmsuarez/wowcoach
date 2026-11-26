//! Offline tests against real API responses saved under
//! `crates/core/tests/fixtures/blizzard/` (US, build 12.1.5, 2026-11-26).
//! The fake API serves: the season index, season 18, a dungeon index with one
//! untracked (542) and one tracked (585, Voidscar Arena) dungeon, Voidscar's
//! journal entry with its 3 bosses, and The Tidebound Grotto raid (1 boss).

use super::*;
use crate::season::Tag;
use std::cell::RefCell;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../tests/fixtures/blizzard/", $name))
    };
}

struct FakeApi {
    calls: RefCell<Vec<String>>,
}

impl FakeApi {
    fn new() -> Self {
        FakeApi {
            calls: RefCell::new(vec![]),
        }
    }
}

impl Api for FakeApi {
    fn get(&self, path: &str, ns: Namespace) -> anyhow::Result<Value> {
        self.calls.borrow_mut().push(path.to_string());
        let (text, want_ns) = match path {
            "/data/wow/mythic-keystone/season/index" => {
                (fixture!("season-index.json"), Namespace::Dynamic)
            }
            "/data/wow/mythic-keystone/season/18" => {
                (fixture!("season-18.json"), Namespace::Dynamic)
            }
            "/data/wow/mythic-keystone/dungeon/index" => {
                (fixture!("dungeon-index.json"), Namespace::Dynamic)
            }
            "/data/wow/mythic-keystone/dungeon/542" => {
                (fixture!("dungeon-542.json"), Namespace::Dynamic)
            }
            "/data/wow/mythic-keystone/dungeon/585" => {
                (fixture!("dungeon-585.json"), Namespace::Dynamic)
            }
            "/data/wow/journal-instance/1313" => {
                (fixture!("journal-instance-1313.json"), Namespace::Static)
            }
            "/data/wow/journal-encounter/2791" => {
                (fixture!("journal-encounter-2791.json"), Namespace::Static)
            }
            "/data/wow/journal-encounter/2792" => {
                (fixture!("journal-encounter-2792.json"), Namespace::Static)
            }
            "/data/wow/journal-encounter/2793" => {
                (fixture!("journal-encounter-2793.json"), Namespace::Static)
            }
            "/data/wow/journal-instance/1317" => {
                (fixture!("journal-instance-1317.json"), Namespace::Static)
            }
            "/data/wow/journal-encounter/2849" => {
                (fixture!("journal-encounter-2849.json"), Namespace::Static)
            }
            _ => anyhow::bail!("unexpected request {path}"),
        };
        assert_eq!(ns, want_ns, "wrong namespace for {path}");
        Ok(serde_json::from_str(text)?)
    }
}

fn synced() -> Synced {
    fetch(&FakeApi::new(), &[1317], |_| {}).unwrap()
}

fn wcl() -> Vec<(String, u32)> {
    vec![
        ("Nymrissa Wavecaller".into(), 3379),
        ("Kith'ix".into(), 3513),
        ("Voidscar Arena".into(), 12923),
    ]
}

fn empty_pack() -> SeasonPack {
    SeasonPack::parse("[season]\nkey = \"midnight-s2\"\nname = \"placeholder\"\n").unwrap()
}

// ------------------------------------------------------------- mapping

#[test]
fn maps_keystone_dungeons() {
    let v: Value = serde_json::from_str(fixture!("dungeon-585.json")).unwrap();
    assert_eq!(
        keystone_dungeon(&v).unwrap(),
        KeystoneDungeon {
            keystone_id: 585,
            name: "Voidscar Arena".into(),
            map_id: 2923,
            journal_id: 1313,
            timer_s: vec![1800, 1440, 1080],
            tracked: true,
        }
    );
    let old: Value = serde_json::from_str(fixture!("dungeon-542.json")).unwrap();
    assert!(!keystone_dungeon(&old).unwrap().tracked);
}

#[test]
fn maps_journal_entries() {
    let v: Value = serde_json::from_str(fixture!("journal-instance-1313.json")).unwrap();
    let j = journal_instance(&v).unwrap();
    assert_eq!(
        (j.journal_id, j.map_id, j.name.as_str()),
        (1313, 2923, "Voidscar Arena")
    );
    let names: Vec<&str> = j.encounters.iter().map(|e| e.1.as_str()).collect();
    assert_eq!(names, ["Taz'Rah", "Atroxus", "Charonus"]);
    assert_eq!(build_version(&v).as_deref(), Some("12.1.5"));

    // Taz'Rah's sections nest spells several levels deep and repeat some.
    let e: Value = serde_json::from_str(fixture!("journal-encounter-2791.json")).unwrap();
    let spells = encounter_spells(&e);
    assert!(spells.contains(&(1300259, "Dark Bloom".into())));
    assert!(
        spells.contains(&(1222098, "Nether Dash".into())),
        "nested 3 levels deep"
    );
    let mut ids: Vec<u32> = spells.iter().map(|s| s.0).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "deduplicated");
}

#[test]
fn build_version_needs_a_static_namespace() {
    assert_eq!(
        build_version(&serde_json::json!({"href": "x?namespace=dynamic-us"})),
        None
    );
    assert_eq!(
        build_version(&serde_json::json!({"href": "x?namespace=static-11.2.0_61000-eu"}))
            .as_deref(),
        Some("11.2.0")
    );
}

// --------------------------------------------------------------- fetch

#[test]
fn fetch_keeps_only_tracked_dungeons() {
    let api = FakeApi::new();
    let mut lines = vec![];
    let s = fetch(&api, &[1317], |l| lines.push(l.to_string())).unwrap();
    assert_eq!(
        (s.season_id, s.season_name.as_str()),
        (18, "Mythic+ Dungeons (Midnight Season 2)")
    );
    assert_eq!(s.build.as_deref(), Some("12.1.5"));
    assert_eq!(s.dungeons.len(), 1);
    assert_eq!(s.dungeons[0].0.name, "Voidscar Arena");
    assert_eq!(s.dungeons[0].1.bosses.len(), 3);
    assert_eq!(s.raids[0].journal.name, "The Tidebound Grotto");
    assert_eq!(s.raids[0].bosses[0].name, "Nymrissa Wavecaller");
    assert!(
        !api.calls
            .borrow()
            .iter()
            .any(|c| c.contains("journal-instance/0"))
    );
    assert_eq!(lines.len(), 2);
}

struct NoTracked;
impl Api for NoTracked {
    fn get(&self, path: &str, ns: Namespace) -> anyhow::Result<Value> {
        match path {
            "/data/wow/mythic-keystone/dungeon/index" => {
                Ok(serde_json::json!({"dungeons": [{"id": 542}]}))
            }
            _ => FakeApi::new().get(path, ns),
        }
    }
}

#[test]
fn empty_pool_is_an_error_not_a_wipe() {
    let e = fetch(&NoTracked, &[], |_| {}).unwrap_err();
    assert!(
        e.to_string().contains("no tracked keystone dungeons"),
        "{e}"
    );
}

// --------------------------------------------------------------- merge

#[test]
fn first_sync_builds_a_valid_pack() {
    let mut pack = empty_pack();
    let sum = merge(&mut pack, &synced(), &wcl());
    assert_eq!(pack.problems(), Vec::<String>::new());
    assert_eq!(pack.season.name, "Mythic+ Dungeons (Midnight Season 2)");
    assert_eq!(pack.season.blizzard_season_id, Some(18));
    assert_eq!(pack.season.synced_build.as_deref(), Some("12.1.5"));

    let d = &pack.dungeons[0];
    assert_eq!((d.keystone_id, d.map_id, d.journal_id), (585, 2923, 1313));
    assert_eq!(d.timer_s, [1800, 1440, 1080]);
    // Journal IDs only; logged IDs for M+ bosses come from `season suggest`.
    assert!(
        d.bosses
            .iter()
            .all(|b| b.journal_id != 0 && b.encounter_id == 0)
    );

    let r = &pack.raids[0];
    assert_eq!((r.name.as_str(), r.map_id), ("The Tidebound Grotto", 2987));
    assert_eq!(r.bosses[0].encounter_id, 3379, "filled from Warcraft Logs");

    assert_eq!(sum.dungeons_added, ["Voidscar Arena"]);
    assert_eq!(sum.raids_added, ["The Tidebound Grotto"]);
    assert_eq!(sum.bosses_added, 4);
    assert_eq!(sum.encounter_ids_filled, 1);
    assert_eq!(sum.spells_added, pack.spells.len());
    assert!(
        pack.spells
            .iter()
            .all(|s| s.sources == [Source::Journal] && s.tags.is_empty())
    );
    let dash = pack
        .spells
        .iter()
        .find(|s| s.name == "Nether Dash")
        .unwrap();
    assert_eq!((dash.map_id, dash.boss.as_deref()), (2923, Some("Taz'Rah")));
}

#[test]
fn resync_is_a_noop_and_byte_identical() {
    let mut pack = empty_pack();
    merge(&mut pack, &synced(), &wcl());
    let first = render(&pack, "").unwrap();
    let mut again = SeasonPack::parse(&first).unwrap();
    let sum = merge(&mut again, &synced(), &wcl());
    assert!(sum.is_noop(), "{sum}");
    assert_eq!(render(&again, &first).unwrap(), first);
}

#[test]
fn merge_never_touches_reviewed_data() {
    let mut pack = SeasonPack::parse(
        r#"
        [season]
        key = "midnight-s2"
        name = "x"
        [[dungeon]]
        name = "Voidscar Arena"
        keystone_id = 585
        map_id = 2923
        bosses = [{ name = "Taz'Rah", encounter_id = 3285 }]
        [[spell]]
        ids = [1222100, 1222103]
        name = "Nether Dash"
        map_id = 2923
        boss = "Taz'Rah"
        sources = ["log"]
        tags = ["avoidable"]
        note = "Step out of the line"
        [[spell]]
        ids = [999001]
        name = "Shadowbolt Volley"
        map_id = 2923
        sources = ["log"]
        suggested = ["priority_kick"]
        "#,
    )
    .unwrap();
    let sum = merge(&mut pack, &synced(), &[]);
    let taz = &pack.dungeons[0].bosses[0];
    assert_eq!(
        (taz.name.as_str(), taz.journal_id, taz.encounter_id),
        ("Taz'Rah", 2791, 3285)
    );

    let dash = pack
        .spells
        .iter()
        .find(|s| s.name == "Nether Dash")
        .unwrap();
    assert_eq!(
        dash.ids,
        [1222098, 1222100, 1222103],
        "journal ID joins the logged ones"
    );
    assert_eq!(dash.tags, [Tag::Avoidable]);
    assert_eq!(dash.note, "Step out of the line");
    assert_eq!(dash.sources, [Source::Journal, Source::Log]);
    assert_eq!(sum.ids_added, 1);

    let volley = pack
        .spells
        .iter()
        .find(|s| s.name == "Shadowbolt Volley")
        .unwrap();
    assert_eq!(volley.suggested, [Tag::PriorityKick]);
    assert_eq!(volley.boss, None, "trash stays trash");
    assert_eq!(pack.problems(), Vec::<String>::new());
}

#[test]
fn dungeon_leaving_the_pool_takes_its_spells() {
    let mut pack = empty_pack();
    merge(&mut pack, &synced(), &[]);
    pack.dungeons.push(Dungeon {
        name: "Old Dungeon".into(),
        keystone_id: 1,
        map_id: 1,
        ..Default::default()
    });
    pack.spells.push(SpellEntry {
        ids: vec![5],
        name: "Old Spell".into(),
        map_id: 1,
        tags: vec![Tag::Avoidable],
        ..Default::default()
    });
    let sum = merge(&mut pack, &synced(), &[]);
    assert_eq!(sum.dungeons_removed, ["Old Dungeon"]);
    assert_eq!(sum.spells_removed, 1);
    assert!(pack.spells.iter().all(|s| s.map_id != 1));
}

#[test]
fn encounter_id_conflicts_are_reported_not_overwritten() {
    let mut pack = empty_pack();
    merge(&mut pack, &synced(), &[]);
    pack.raids[0].bosses[0].encounter_id = 1111;
    let sum = merge(&mut pack, &synced(), &wcl());
    assert_eq!(pack.raids[0].bosses[0].encounter_id, 1111);
    assert_eq!(
        sum.encounter_id_conflicts,
        [("Nymrissa Wavecaller".to_string(), 1111, 3379)]
    );
    assert!(sum.to_string().contains("kept 1111"));
}

#[test]
fn render_keeps_the_header_comment() {
    let mut pack = empty_pack();
    merge(&mut pack, &synced(), &[]);
    let prev = "# Season data pack\n# Tags: ...\n\n[season]\nkey = \"old\"\n";
    let out = render(&pack, prev).unwrap();
    assert!(
        out.starts_with("# Season data pack\n# Tags: ...\n\n[season]\n"),
        "{}",
        &out[..80]
    );
    SeasonPack::parse(&out).unwrap();
}

#[test]
fn same_name_on_another_boss_stays_separate() {
    let boss = |name: &str, id: u32, spells: &[(u32, &str)]| SyncedBoss {
        journal_id: id,
        name: name.into(),
        spells: spells.iter().map(|(i, n)| (*i, n.to_string())).collect(),
    };
    let mut s = synced();
    s.raids[0].bosses = vec![boss(
        "Nymrissa Wavecaller",
        2849,
        &[(1, "Berserk"), (2, "Tidal Wave"), (3, "Tidal Wave")],
    )];
    s.raids[0]
        .journal
        .encounters
        .push((9999, "Second Boss".into()));
    s.raids[0]
        .bosses
        .push(boss("Second Boss", 9999, &[(4, "Berserk")]));
    let mut pack = empty_pack();
    merge(&mut pack, &s, &[]);
    let named = |n: &str| -> Vec<Vec<u32>> {
        pack.spells
            .iter()
            .filter(|x| x.name == n)
            .map(|x| x.ids.clone())
            .collect()
    };
    assert_eq!(
        named("Tidal Wave"),
        [vec![2, 3]],
        "same boss, same name: one mechanic"
    );
    assert_eq!(
        named("Berserk"),
        [vec![1], vec![4]],
        "different bosses: kept apart"
    );
    assert_eq!(pack.problems(), Vec::<String>::new());
}
