use super::*;
use crate::model::{REACTION_HOSTILE, TYPE_NPC};
use crate::testutil::*;

/// Trimmed from the shape of an MDT `Midnight/*.lua` file.
const MDT_FILE: &str = r#"
local _, MDT = ...
local addonName = MDT.AddonName
local L = MDT.L
local dungeonIndex = 161
MDT.dungeonList[dungeonIndex] = L["DenOfNalorakk"]
MDT.mapInfo[dungeonIndex] = {
  teleportId = 1286807,
  shortName = L["DenOfNalorakkShortName"],
  englishName = "Den of Nalorakk",
  mapID = 586
};

MDT:RegisterDungeonLocation(dungeonIndex, {
  zoneIds = { 2437, 2513 },
  subzoneAreaIDs = { 16189 }, -- Nalorakk's Prowl
})

MDT.dungeonMaps[dungeonIndex] = {
  [0] = "",
  [1] = { customTextures = 'Interface\\AddOns\\'..addonName..'\\Midnight\\Textures\\DenOfNalorakk' },
}
--[[ a block
comment ]]
MDT.dungeonTotalCount[dungeonIndex] = { normal = 729 }

MDT.dungeonEnemies[dungeonIndex] = {
  [1] = {
    ["name"] = "Frostfang",
    ["id"] = 241874,
    ["count"] = 5,
    ["health"] = 1234567,
    ["scale"] = 1.2,
    ["spells"] = {
      [1246957] = {
      },
    },
    ["clones"] = {
      [1] = {
        ["x"] = 683.09818616262,
        ["y"] = -236.62020459855,
        ["g"] = 68,
        ["sublevel"] = 1,
      },
    },
  },
  [2] = {
    ["name"] = "Grizzled Warbringer",
    ["id"] = 245148,
    ["count"] = 0,
  },
  [3] = {
    ["name"] = "The Winter Squall",
    ["id"] = 250478,
    ["count"] = 50,
  },
  [4] = {
    ["name"] = "Nalorakk",
    ["id"] = 240000,
    ["count"] = 0,
    ["isBoss"] = true,
  },
  [5] = {
    ["name"] = "Frostfang",
    ["id"] = 241874,
    ["count"] = 4,
  },
};
"#;

fn pack() -> SeasonPack {
    SeasonPack::parse(
        r#"
        [season]
        key = "t"
        name = "T"
        [[dungeon]]
        name = "Den of Nalorakk"
        keystone_id = 586
        map_id = 2825
        [[dungeon]]
        name = "Murder Row"
        keystone_id = 587
        map_id = 2813
        [[npc]]
        id = 999
        name = "Gone From MDT"
        map_id = 2825
        forces = 3
        source = "mdt"
        [[npc]]
        id = 250478
        name = "The Winter Squall"
        map_id = 2825
        forces = 40
        source = "manual"
        note = "checked in game"
        "#,
    )
    .unwrap()
}

#[test]
fn parses_an_mdt_dungeon_file() {
    let d = parse_mdt(MDT_FILE).unwrap().unwrap();
    assert_eq!(
        (d.keystone_id, d.name.as_str(), d.total),
        (586, "Den of Nalorakk", 729)
    );
    assert_eq!(d.enemies.len(), 5);
    assert_eq!(
        d.enemies[3],
        MdtEnemy {
            id: 240000,
            name: "Nalorakk".into(),
            count: 0,
            is_boss: true
        }
    );
    assert!(parse_mdt("local x = 1").unwrap().is_none());
}

#[test]
fn import_only_touches_mdt_entries() {
    let mut p = pack();
    let d = parse_mdt(MDT_FILE).unwrap().unwrap();
    let r = import(&mut p, std::slice::from_ref(&d));
    assert_eq!(
        p.dungeons
            .iter()
            .find(|x| x.map_id == 2825)
            .unwrap()
            .forces_total,
        729
    );
    // Frostfang (listed twice: the higher count wins) is added; the 0-forces
    // Warbringer and the boss are not; the manual Squall keeps its 40; the
    // MDT entry MDT no longer lists is removed.
    let got: Vec<(u32, u32, Source)> = p.npcs.iter().map(|n| (n.id, n.forces, n.source)).collect();
    assert_eq!(
        got,
        vec![(241874, 5, Source::Mdt), (250478, 40, Source::Manual)]
    );
    assert_eq!((r.added, r.updated, r.removed), (1, 0, 1));
    assert_eq!(r.missing, vec!["Murder Row".to_string()]);
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    // A second import changes nothing.
    let again = import(&mut p, &[d]);
    assert_eq!((again.added, again.updated, again.removed), (0, 0, 0));
}

fn mob(name: &str, npc: u32, spawn: u32) -> Unit {
    Unit {
        guid: format!("Creature-0-1-2825-1-{npc}-{spawn:010X}"),
        name: name.into(),
        flags: TYPE_NPC | REACTION_HOSTILE,
    }
}

fn den_pack() -> SeasonPack {
    let mut p = SeasonPack::parse(
        r#"
        [season]
        key = "t"
        name = "T"
        [[dungeon]]
        name = "Den of Nalorakk"
        keystone_id = 586
        map_id = 2825
        forces_total = 20
        [[npc]]
        id = 100
        name = "Brute"
        map_id = 2825
        forces = 5
        source = "mdt"
        [[npc]]
        id = 200
        name = "Shaman"
        map_id = 2825
        forces = 4
        source = "mdt"
        [[npc]]
        id = 300
        name = "Shadow"
        map_id = 2825
        forces = 8
        source = "mdt"
        "#,
    )
    .unwrap();
    crate::season::sort(&mut p);
    p
}

/// A completed key: two Brutes die, two Shamans die under another ID, and a
/// Shadow is beaten to 1 HP and never dies (scripted away). A Brute that
/// dies twice counts once. Then an abandoned key, which is ignored.
fn den_log(shadow_hp: u64) -> Log {
    let p = player("Alpha");
    let mut log = Log::default();
    log.key_start(0, "Den of Nalorakk", 2825, 12);
    for i in 0..2 {
        log.death(1000 + i, &mob("Brute", 100, i as u32));
    }
    log.death(1500, &mob("Brute", 100, 0));
    for i in 0..2 {
        let s = mob("Shaman", 201, i);
        log.damage(2000, &p, &s, 1, "Melee", 10).death(2100, &s);
    }
    log.damage_hp(
        3000,
        &p,
        &mob("Shadow", 300, 9),
        1,
        "Melee",
        10,
        Some((shadow_hp, 1000)),
    );
    log.key_end(4000, 2825, true, 4000);
    log.key_start(5000, "Den of Nalorakk", 2825, 12)
        .death(5100, &mob("Brute", 100, 7))
        .key_end(5200, 2825, false, 0);
    log
}

fn check(log: &Log, p: &SeasonPack) -> Check {
    let mut a = Audit::default();
    for e in &log.events {
        a.feed(e);
    }
    a.report(p)
}

#[test]
fn check_counts_kills_and_units_defeated_at_one_hp() {
    let p = den_pack();
    let c = check(&den_log(10), &p);
    // Brutes 2 × 5 + Shadow 8 (1% HP, no death); the Shamans aren't known yet.
    assert_eq!(c.keys.len(), 1);
    assert_eq!((c.keys[0].forces, c.keys[0].total), (18, 20));
    // 18 < 20: the Shamans died as 201, a same-name NPC with no forces.
    assert_eq!(c.keys[0].with_aliases, 26);
    assert_eq!(
        c.aliases,
        vec![Alias {
            dungeon: "Den of Nalorakk".into(),
            map_id: 2825,
            logged_id: 201,
            name: "Shaman".into(),
            table_id: 200,
            forces: 4,
            keys: 1,
        }]
    );
    assert!(c.missing.is_empty());

    // At 2% the Shadow wasn't defeated: it's listed as never dying.
    let c = check(&den_log(20), &p);
    assert_eq!(c.keys[0].forces, 10);
    assert_eq!(
        c.missing
            .iter()
            .map(|m| (m.id, m.seen_alive))
            .collect::<Vec<_>>(),
        vec![(300, 1)]
    );
}

#[test]
fn check_aliases_are_written_for_review_and_then_cover_the_npc() {
    let mut p = den_pack();
    let c = check(&den_log(10), &p);
    assert_eq!(c.apply(&mut p), 1);
    let n = p.npcs.iter().find(|n| n.id == 201).unwrap();
    assert_eq!((n.forces, n.source), (4, Source::Log));
    assert!(n.note.contains("MDT's 200"), "{}", n.note);
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    // With the alias in the table, the key is complete and nothing is proposed.
    let c = check(&den_log(10), &p);
    assert_eq!(c.keys[0].forces, 26);
    assert!(c.aliases.is_empty() && c.missing.is_empty());
    assert_eq!(c.apply(&mut p), 0);
}
