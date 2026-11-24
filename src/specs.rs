use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Role {
    Tank,
    Healer,
    Dps,
}

/// (class, spec, role) for a specialization ID from COMBATANT_INFO.
pub fn spec_info(id: u32) -> Option<(&'static str, &'static str, Role)> {
    use Role::*;
    Some(match id {
        71 => ("Warrior", "Arms", Dps),
        72 => ("Warrior", "Fury", Dps),
        73 => ("Warrior", "Protection", Tank),
        65 => ("Paladin", "Holy", Healer),
        66 => ("Paladin", "Protection", Tank),
        70 => ("Paladin", "Retribution", Dps),
        253 => ("Hunter", "Beast Mastery", Dps),
        254 => ("Hunter", "Marksmanship", Dps),
        255 => ("Hunter", "Survival", Dps),
        259 => ("Rogue", "Assassination", Dps),
        260 => ("Rogue", "Outlaw", Dps),
        261 => ("Rogue", "Subtlety", Dps),
        256 => ("Priest", "Discipline", Healer),
        257 => ("Priest", "Holy", Healer),
        258 => ("Priest", "Shadow", Dps),
        250 => ("Death Knight", "Blood", Tank),
        251 => ("Death Knight", "Frost", Dps),
        252 => ("Death Knight", "Unholy", Dps),
        262 => ("Shaman", "Elemental", Dps),
        263 => ("Shaman", "Enhancement", Dps),
        264 => ("Shaman", "Restoration", Healer),
        62 => ("Mage", "Arcane", Dps),
        63 => ("Mage", "Fire", Dps),
        64 => ("Mage", "Frost", Dps),
        265 => ("Warlock", "Affliction", Dps),
        266 => ("Warlock", "Demonology", Dps),
        267 => ("Warlock", "Destruction", Dps),
        268 => ("Monk", "Brewmaster", Tank),
        269 => ("Monk", "Windwalker", Dps),
        270 => ("Monk", "Mistweaver", Healer),
        102 => ("Druid", "Balance", Dps),
        103 => ("Druid", "Feral", Dps),
        104 => ("Druid", "Guardian", Tank),
        105 => ("Druid", "Restoration", Healer),
        577 => ("Demon Hunter", "Havoc", Dps),
        581 => ("Demon Hunter", "Vengeance", Tank),
        1480 => ("Demon Hunter", "Devourer", Dps),
        1467 => ("Evoker", "Devastation", Dps),
        1468 => ("Evoker", "Preservation", Healer),
        1473 => ("Evoker", "Augmentation", Dps),
        _ => return None,
    })
}
