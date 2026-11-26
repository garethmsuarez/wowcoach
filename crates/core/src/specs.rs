use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Role {
    Tank,
    Healer,
    Dps,
}

/// Every specialization: (spec ID from COMBATANT_INFO, class, spec, role).
pub const SPECS: &[(u32, &str, &str, Role)] = &[
    (71, "Warrior", "Arms", Role::Dps),
    (72, "Warrior", "Fury", Role::Dps),
    (73, "Warrior", "Protection", Role::Tank),
    (65, "Paladin", "Holy", Role::Healer),
    (66, "Paladin", "Protection", Role::Tank),
    (70, "Paladin", "Retribution", Role::Dps),
    (253, "Hunter", "Beast Mastery", Role::Dps),
    (254, "Hunter", "Marksmanship", Role::Dps),
    (255, "Hunter", "Survival", Role::Dps),
    (259, "Rogue", "Assassination", Role::Dps),
    (260, "Rogue", "Outlaw", Role::Dps),
    (261, "Rogue", "Subtlety", Role::Dps),
    (256, "Priest", "Discipline", Role::Healer),
    (257, "Priest", "Holy", Role::Healer),
    (258, "Priest", "Shadow", Role::Dps),
    (250, "Death Knight", "Blood", Role::Tank),
    (251, "Death Knight", "Frost", Role::Dps),
    (252, "Death Knight", "Unholy", Role::Dps),
    (262, "Shaman", "Elemental", Role::Dps),
    (263, "Shaman", "Enhancement", Role::Dps),
    (264, "Shaman", "Restoration", Role::Healer),
    (62, "Mage", "Arcane", Role::Dps),
    (63, "Mage", "Fire", Role::Dps),
    (64, "Mage", "Frost", Role::Dps),
    (265, "Warlock", "Affliction", Role::Dps),
    (266, "Warlock", "Demonology", Role::Dps),
    (267, "Warlock", "Destruction", Role::Dps),
    (268, "Monk", "Brewmaster", Role::Tank),
    (269, "Monk", "Windwalker", Role::Dps),
    (270, "Monk", "Mistweaver", Role::Healer),
    (102, "Druid", "Balance", Role::Dps),
    (103, "Druid", "Feral", Role::Dps),
    (104, "Druid", "Guardian", Role::Tank),
    (105, "Druid", "Restoration", Role::Healer),
    (577, "Demon Hunter", "Havoc", Role::Dps),
    (581, "Demon Hunter", "Vengeance", Role::Tank),
    (1480, "Demon Hunter", "Devourer", Role::Dps),
    (1467, "Evoker", "Devastation", Role::Dps),
    (1468, "Evoker", "Preservation", Role::Healer),
    (1473, "Evoker", "Augmentation", Role::Dps),
];

/// (class, spec, role) for a specialization ID from COMBATANT_INFO.
pub fn spec_info(id: u32) -> Option<(&'static str, &'static str, Role)> {
    SPECS.iter().find(|s| s.0 == id).map(|s| (s.1, s.2, s.3))
}

/// Spec names of a class, in table order. Empty for an unknown class.
pub fn class_specs(class: &str) -> Vec<&'static str> {
    SPECS.iter().filter(|s| s.1 == class).map(|s| s.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(
            spec_info(1480),
            Some(("Demon Hunter", "Devourer", Role::Dps))
        );
        assert_eq!(spec_info(1), None);
        assert_eq!(
            class_specs("Druid"),
            ["Balance", "Feral", "Guardian", "Restoration"]
        );
        let mut classes: Vec<&str> = SPECS.iter().map(|s| s.1).collect();
        classes.dedup();
        assert_eq!(classes.len(), 13);
    }
}
