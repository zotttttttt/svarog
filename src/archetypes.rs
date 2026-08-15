use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchetypeId {
    Boxer,
    Wrestler,
    MartialArtist,
    Bodybuilder,
    Runner,
    #[default]
    Athlete,
    Gymnast,
    Yogi,
    Mover,
    Thinker,
    Lifer,
    Custom,
}

impl ArchetypeId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boxer => "boxer",
            Self::Wrestler => "wrestler",
            Self::MartialArtist => "martial_artist",
            Self::Bodybuilder => "bodybuilder",
            Self::Runner => "runner",
            Self::Athlete => "athlete",
            Self::Gymnast => "gymnast",
            Self::Yogi => "yogi",
            Self::Mover => "mover",
            Self::Thinker => "thinker",
            Self::Lifer => "lifer",
            Self::Custom => "custom",
        }
    }
}

impl FromStr for ArchetypeId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        BUILT_INS
            .iter()
            .find(|archetype| archetype.id.as_str() == value.trim().to_lowercase())
            .map(|archetype| archetype.id)
            .or_else(|| (value.trim().eq_ignore_ascii_case("custom")).then_some(Self::Custom))
            .ok_or_else(|| "unknown Forge archetype".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchetypeStats {
    pub strength: u8,
    pub muscle: u8,
    pub cardio: u8,
    pub mobility: u8,
    pub control: u8,
    pub stamina: u8,
    pub longevity: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Archetype {
    pub id: ArchetypeId,
    pub name: &'static str,
    pub description: &'static str,
    pub stats: ArchetypeStats,
    pub preferred_categories: &'static [&'static str],
    pub preferred_forces: &'static [&'static str],
    pub preferred_mechanics: &'static [&'static str],
    pub preferred_muscles: &'static [&'static str],
}

macro_rules! archetype {
    ($id:ident, $name:literal, $description:literal,
     [$str:literal,$mus:literal,$car:literal,$mob:literal,$ctl:literal,$sta:literal,$lon:literal],
     $categories:expr, $forces:expr, $mechanics:expr, $muscles:expr) => {
        Archetype {
            id: ArchetypeId::$id,
            name: $name,
            description: $description,
            stats: ArchetypeStats {
                strength: $str,
                muscle: $mus,
                cardio: $car,
                mobility: $mob,
                control: $ctl,
                stamina: $sta,
                longevity: $lon,
            },
            preferred_categories: $categories,
            preferred_forces: $forces,
            preferred_mechanics: $mechanics,
            preferred_muscles: $muscles,
        }
    };
}

pub const BUILT_INS: [Archetype; 11] = [
    archetype!(Boxer, "Boxer", "Fast, conditioned and durable: calisthenics, footwork, core work and very high distributed training volume.", [6,5,9,6,8,10,7], &["cardio", "plyometrics", "strength"], &["push"], &["compound"], &["abdominals", "shoulders", "quadriceps"]),
    archetype!(Wrestler, "Wrestler", "Powerful and hard to fatigue: pulling, grip, posterior chain, carries, isometrics and explosive full-body strength.", [9,8,8,6,8,9,7], &["strongman", "strength", "powerlifting"], &["pull"], &["compound"], &["forearms", "lats", "hamstrings", "lower back", "glutes"]),
    archetype!(MartialArtist, "Martial Artist", "Lean, mobile and precise: speed, coordination, balance, core control and explosive movement.", [7,5,7,10,10,8,8], &["plyometrics", "stretching", "cardio"], &[], &["compound"], &["abdominals", "adductors", "abductors"]),
    archetype!(Bodybuilder, "Bodybuilder", "Build muscle deliberately through resistance, progressive overload and hypertrophy-focused strength work.", [9,10,4,4,6,6,6], &["strength"], &["push", "pull"], &["isolation"], &["chest", "biceps", "triceps", "shoulders", "quadriceps"]),
    archetype!(Runner, "Runner", "Build a large aerobic engine through movement volume, cardiovascular fitness and durable lower-body endurance.", [4,3,10,5,6,10,8], &["cardio", "plyometrics"], &[], &["compound"], &["calves", "quadriceps", "hamstrings", "glutes"]),
    archetype!(Athlete, "Athlete", "Be good at everything: balanced strength, muscle, cardio, mobility, coordination and power.", [8,7,8,8,8,8,8], &["strength", "cardio", "stretching", "plyometrics"], &["push", "pull"], &["compound"], &[]),
    archetype!(Gymnast, "Gymnast", "Master your own body through relative strength, core control, balance, mobility and precise movement.", [8,7,7,10,10,8,8], &["strength", "stretching"], &["push", "pull", "static"], &["compound"], &["abdominals", "lats", "shoulders"]),
    archetype!(Yogi, "Yogi", "Move freely and deliberately through mobility, flexibility, balance, breathing and controlled body awareness.", [3,3,4,10,9,5,9], &["stretching"], &["static"], &[], &["hamstrings", "lower back", "adductors", "abductors"]),
    archetype!(Mover, "Mover", "Strong posture and controlled movement through core work, mobility, balance and low-impact muscular endurance.", [5,5,5,9,9,6,9], &["stretching", "strength"], &["static"], &["isolation"], &["abdominals", "glutes", "lower back"]),
    archetype!(Thinker, "Thinker", "Use movement to improve focus, energy, mood, stress regulation, sleep and cognitive performance.", [4,3,6,7,7,6,9], &["cardio", "stretching"], &[], &[], &["neck", "shoulders", "lower back"]),
    archetype!(Lifer, "Lifer", "Stay strong, mobile, aerobically fit and physically independent for as many decades as possible.", [7,6,8,8,8,7,10], &["strength", "cardio", "stretching"], &["pull"], &["compound"], &["quadriceps", "hamstrings", "glutes", "forearms"]),
];

pub fn get(id: ArchetypeId) -> &'static Archetype {
    let effective = if id == ArchetypeId::Custom {
        ArchetypeId::Athlete
    } else {
        id
    };
    BUILT_INS.iter().find(|item| item.id == effective).unwrap()
}

pub fn display_name(id: ArchetypeId, custom_name: Option<&str>) -> Cow<'static, str> {
    if id == ArchetypeId::Custom {
        custom_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| Cow::Owned(name.to_string()))
            .unwrap_or(Cow::Borrowed("Custom"))
    } else {
        Cow::Borrowed(get(id).name)
    }
}

pub fn index(id: ArchetypeId) -> usize {
    BUILT_INS.iter().position(|item| item.id == id).unwrap_or(5)
}

pub fn next(id: ArchetypeId, delta: isize) -> ArchetypeId {
    let len = BUILT_INS.len() as isize;
    BUILT_INS[((index(id) as isize + delta).rem_euclid(len)) as usize].id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_stable_complete_stats_and_wraps() {
        assert_eq!(BUILT_INS.len(), 11);
        assert!(BUILT_INS.iter().all(|item| [
            item.stats.strength,
            item.stats.muscle,
            item.stats.cardio,
            item.stats.mobility,
            item.stats.control,
            item.stats.stamina,
            item.stats.longevity
        ]
        .into_iter()
        .all(|score| (1..=10).contains(&score))));
        assert_eq!(next(ArchetypeId::Boxer, -1), ArchetypeId::Lifer);
        assert_eq!(next(ArchetypeId::Lifer, 1), ArchetypeId::Boxer);
        assert_eq!(get(ArchetypeId::Custom).id, ArchetypeId::Athlete);
        assert_eq!(
            BUILT_INS
                .iter()
                .map(|item| [
                    item.stats.strength,
                    item.stats.muscle,
                    item.stats.cardio,
                    item.stats.mobility,
                    item.stats.control,
                    item.stats.stamina,
                    item.stats.longevity,
                ])
                .collect::<Vec<_>>(),
            vec![
                [6, 5, 9, 6, 8, 10, 7],
                [9, 8, 8, 6, 8, 9, 7],
                [7, 5, 7, 10, 10, 8, 8],
                [9, 10, 4, 4, 6, 6, 6],
                [4, 3, 10, 5, 6, 10, 8],
                [8, 7, 8, 8, 8, 8, 8],
                [8, 7, 7, 10, 10, 8, 8],
                [3, 3, 4, 10, 9, 5, 9],
                [5, 5, 5, 9, 9, 6, 9],
                [4, 3, 6, 7, 7, 6, 9],
                [7, 6, 8, 8, 8, 7, 10],
            ]
        );
    }

    #[test]
    fn custom_display_name_does_not_use_behavior_fallback() {
        assert_eq!(display_name(ArchetypeId::Custom, Some("Goku")), "Goku");
        assert_eq!(display_name(ArchetypeId::Custom, None), "Custom");
        assert_eq!(display_name(ArchetypeId::Boxer, Some("ignored")), "Boxer");
    }
}
