use crate::models::{Movement, MovementSidedness, MovementStatus};
#[cfg(test)]
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::OnceLock;

const CATALOG_JSON: &str = include_str!("../data/free-exercise-db.compact.json");
pub const CATALOG_REVISION: &str = "b0eed061e1c832b3ed815fbaa4b45b3cdc14df49";
const MOVEMENT_POOL_POLICY_REVISION: &str = "equipment-v2";
const PARTNER_REQUIRED_EXERCISE_IDS: &[&str] = &[
    "Adductor_Groin",
    "Barbell_Seated_Calf_Raise",
    "Behind_Head_Chest_Stretch",
    "Cable_Preacher_Curl",
    "Cable_Seated_Lateral_Raise",
    "Flat_Bench_Cable_Flyes",
    "Hyperextensions_With_No_Hyperextension_Bench",
    "Lying_Bent_Leg_Groin",
    "Lying_Crossover",
    "Lying_Glute",
    "Lying_Hamstring",
    "Lying_Prone_Quadriceps",
    "Medicine_Ball_Full_Twist",
    "One_Arm_Floor_Press",
    "Overhead_Lat",
    "Overhead_Triceps",
    "Prone_Manual_Hamstring",
    "Return_Push_from_Stance",
    "Seated_Biceps",
    "Seated_Front_Deltoid",
    "Seated_Glute",
    "Seated_Hamstring",
    "Standing_Towel_Triceps_Extension",
    "Weighted_Bench_Dip",
];

const PULL_UP_BAR_EXERCISE_IDS: &[&str] = &[
    "Chin-Up",
    "Gorilla_Chin_Crunch",
    "Hanging_Leg_Raise",
    "Hanging_Pike",
    "Pullups",
    "V-Bar_Pullup",
    "Wide-Grip_Rear_Pull-Up",
    "Wind_Sprints",
];

const BENCH_OR_BOX_EXERCISE_IDS: &[&str] = &[
    "Bench_Dips",
    "Bench_Jump",
    "Decline_Crunch",
    "Decline_Oblique_Crunch",
    "Decline_Reverse_Crunch",
    "Flat_Bench_Lying_Leg_Raise",
    "Flutter_Kicks",
    "Incline_Push-Up",
    "Incline_Push-Up_Close-Grip",
    "Incline_Push-Up_Medium",
    "Incline_Push-Up_Reverse_Grip",
    "Incline_Push-Up_Wide",
    "Push-Ups_With_Feet_Elevated",
    "Seated_Flat_Bench_Leg_Pull-In",
    "Seated_Leg_Tucks",
    "Step-up_with_Knee_Raise",
];

const STABLE_SUPPORT_EXERCISE_IDS: &[&str] =
    &["Front_Leg_Raises", "Leg_Lift", "Standing_Hip_Circles"];

const DOUBLE_KETTLEBELL_EXERCISE_IDS: &[&str] = &[
    "Alternating_Floor_Press",
    "Alternating_Hang_Clean",
    "Alternating_Kettlebell_Press",
    "Alternating_Kettlebell_Row",
    "Alternating_Renegade_Row",
    "Double_Kettlebell_Alternating_Hang_Clean",
    "Double_Kettlebell_Jerk",
    "Double_Kettlebell_Push_Press",
    "Double_Kettlebell_Snatch",
    "Double_Kettlebell_Windmill",
    "Front_Squats_With_Two_Kettlebells",
    "Kettlebell_Seesaw_Press",
    "Kettlebell_Thruster",
    "Two-Arm_Kettlebell_Clean",
    "Two-Arm_Kettlebell_Jerk",
    "Two-Arm_Kettlebell_Military_Press",
    "Two-Arm_Kettlebell_Row",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EquipmentRequirement {
    pub(crate) kind: &'static str,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseCatalogEntry {
    pub id: String,
    pub force: Option<String>,
    pub mechanic: Option<String>,
    pub equipment: Option<String>,
    pub primary_muscles: Vec<String>,
    pub secondary_muscles: Vec<String>,
    pub category: String,
    #[serde(default, skip_serializing)]
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing)]
    pub images: Vec<String>,
}

pub fn all() -> &'static [ExerciseCatalogEntry] {
    static CATALOG: OnceLock<Vec<ExerciseCatalogEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut catalog: Vec<ExerciseCatalogEntry> = serde_json::from_str(CATALOG_JSON)
            .expect("bundled exercise catalog must be valid JSON");
        catalog.retain(|entry| !PARTNER_REQUIRED_EXERCISE_IDS.contains(&entry.id.as_str()));
        catalog
    })
}

pub fn movement_pool_revision() -> &'static str {
    static REVISION: OnceLock<String> = OnceLock::new();
    REVISION.get_or_init(|| format!("{CATALOG_REVISION}:{MOVEMENT_POOL_POLICY_REVISION}"))
}

pub fn find(id: &str) -> Option<&'static ExerciseCatalogEntry> {
    all().iter().find(|entry| entry.id == id)
}

#[cfg(test)]
pub fn candidates(equipment_text: &str) -> Vec<ExerciseCatalogEntry> {
    candidates_for_equipment(&locally_resolved_equipment(equipment_text))
}

pub fn candidates_for_equipment(available: &[String]) -> Vec<ExerciseCatalogEntry> {
    let available = equipment_counts(available);
    all()
        .iter()
        .filter(|entry| {
            equipment_requirements(entry).is_some_and(|requirements| {
                requirements.iter().all(|required| {
                    available.get(required.kind).copied().unwrap_or(0) >= required.count
                })
            })
        })
        .cloned()
        .collect()
}

pub fn equipment_text_matches_entry(text: &str, entry: &ExerciseCatalogEntry) -> bool {
    let available = locally_resolved_equipment(text);
    let available = equipment_counts(&available);
    equipment_requirements(entry).is_some_and(|requirements| {
        requirements
            .iter()
            .all(|required| available.get(required.kind).copied().unwrap_or(0) >= required.count)
    })
}

fn equipment_counts(available: &[String]) -> HashMap<&str, usize> {
    available.iter().fold(HashMap::new(), |mut counts, kind| {
        *counts.entry(kind.as_str()).or_insert(0usize) += 1;
        counts
    })
}

pub fn movements_for_equipment(available: &[String]) -> Vec<Movement> {
    candidates_for_equipment(available)
        .iter()
        .map(|entry| {
            let (reps, seconds) = prescription_defaults(entry);
            movement_from_entry(
                entry,
                reps,
                seconds,
                MovementStatus::Allowed,
                inferred_sidedness(&entry.id),
            )
        })
        .collect()
}

pub fn entries_for_movements(movements: &[Movement]) -> Vec<ExerciseCatalogEntry> {
    movements
        .iter()
        .filter(|movement| movement.status != MovementStatus::Blocked)
        .filter_map(|movement| find(&movement.id).cloned())
        .collect()
}

pub fn movement_from_entry(
    entry: &ExerciseCatalogEntry,
    base_reps: u32,
    estimated_seconds: u32,
    status: MovementStatus,
    sidedness: MovementSidedness,
) -> Movement {
    let mut muscles = entry.primary_muscles.clone();
    for muscle in &entry.secondary_muscles {
        if !muscles.contains(muscle) {
            muscles.push(muscle.clone());
        }
    }
    Movement {
        id: entry.id.clone(),
        name: display_name(&entry.id),
        primary_muscle: entry
            .primary_muscles
            .first()
            .cloned()
            .unwrap_or_else(|| "full body".to_string()),
        muscles,
        equipment: entry
            .equipment
            .as_deref()
            .and_then(normalize_equipment)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        base_reps,
        estimated_seconds,
        status,
        mobility: entry.category == "stretching",
        sidedness,
    }
}

pub fn locally_resolved_equipment(text: &str) -> Vec<String> {
    let mut equipment = available_equipment(text)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    equipment.sort();
    equipment
}

pub fn prescription_defaults(entry: &ExerciseCatalogEntry) -> (u32, u32) {
    match entry.category.as_str() {
        "stretching" => (4, 35),
        "plyometrics" | "olympic weightlifting" | "powerlifting" => (5, 45),
        "strongman" => (1, 35),
        _ if entry.force.as_deref() == Some("static") => (1, 30),
        _ => (8, 45),
    }
}

pub fn inferred_sidedness(id: &str) -> MovementSidedness {
    let id = id.to_lowercase();
    if [
        "one-arm",
        "one_arm",
        "single-arm",
        "single_arm",
        "alternate",
        "alternating",
    ]
    .iter()
    .any(|needle| id.contains(needle))
    {
        MovementSidedness::Unilateral
    } else {
        MovementSidedness::Bilateral
    }
}

#[cfg(test)]
pub fn validate() -> Result<()> {
    let mut ids = HashSet::new();
    for entry in all() {
        if entry.id.trim().is_empty() || !ids.insert(&entry.id) {
            anyhow::bail!("exercise catalog contains an empty or duplicate id");
        }
        if entry.category.trim().is_empty() {
            anyhow::bail!("exercise {} has no category", entry.id);
        }
        if entry.images.iter().any(|image| image.trim().is_empty()) {
            anyhow::bail!("exercise {} has an empty image path", entry.id);
        }
    }
    serde_json::from_str::<Vec<ExerciseCatalogEntry>>(CATALOG_JSON)
        .context("validating bundled exercise catalog")?;
    Ok(())
}

fn available_equipment(text: &str) -> Vec<&'static str> {
    let text = text.to_lowercase();
    let mut available = vec!["bodyweight"];
    for (kind, aliases) in [
        ("dumbbell", &["dumbbell", "dumb bell"][..]),
        ("kettlebell", &["kettlebell", "kettle bell"][..]),
        ("band", &["resistance band", "bands", "band"][..]),
        ("medicine_ball", &["medicine ball", "medical ball"][..]),
        ("barbell", &["barbell"][..]),
        ("e_z_curl_bar", &["e-z curl bar", "ez curl bar"][..]),
        ("exercise_ball", &["exercise ball", "stability ball"][..]),
        ("foam_roll", &["foam roll", "foam roller"][..]),
        ("cable", &["cable machine", "cable"][..]),
        ("machine", &["gym machine", "machines"][..]),
        (
            "pull_up_bar",
            &[
                "pull-up bar",
                "pull up bar",
                "pullup bar",
                "chin-up bar",
                "chin up bar",
                "chinup bar",
                "doorway bar",
                "door-frame bar",
                "door frame bar",
            ][..],
        ),
        ("v_bar", &["v-bar", "v bar", "neutral grip handle"][..]),
        (
            "bench_or_box",
            &[
                "bench",
                "plyo box",
                "plyometric box",
                "exercise box",
                "elevated platform",
                "workout step",
            ][..],
        ),
        (
            "dip_station",
            &["dip station", "dip bars", "parallel bars"][..],
        ),
        ("rack", &["squat rack", "power rack", "barbell rack"][..]),
        ("wall", &["wall"][..]),
        (
            "stable_support",
            &[
                "chair",
                "railing",
                "counter",
                "vertical support",
                "sturdy support",
            ][..],
        ),
        (
            "leg_anchor",
            &[
                "lat pulldown",
                "lat-pulldown",
                "preacher bench",
                "leg anchor",
            ][..],
        ),
    ] {
        let matched = aliases.iter().any(|alias| text.contains(alias))
            || (kind == "bench_or_box"
                && text
                    .split(|ch: char| !ch.is_ascii_alphanumeric())
                    .any(|word| matches!(word, "box" | "platform" | "step")));
        if matched {
            let count = if matches!(kind, "kettlebell" | "dumbbell") {
                repeated_equipment_count(&text, kind)
            } else {
                1
            };
            available.extend(std::iter::repeat_n(kind, count));
        }
    }
    available
}

fn repeated_equipment_count(text: &str, kind: &str) -> usize {
    let singular = kind;
    let plural = format!("{kind}s");
    let words = text
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let mut count = 1;
    for (index, word) in words.iter().enumerate() {
        if *word != singular && *word != plural {
            continue;
        }
        if *word == plural {
            count = count.max(2);
        }
        if let Some(previous) = index.checked_sub(1).and_then(|at| words.get(at)) {
            if let Ok(explicit) = previous.parse::<usize>() {
                count = count.max(explicit);
            }
            if matches!(*previous, "two" | "pair" | "double") {
                count = count.max(2);
            }
        }
        if index >= 2 && words[index - 1] == "of" && words[index - 2] == "pair" {
            count = count.max(2);
        }
        for nearby in words[index.saturating_sub(3)..index].iter() {
            if let Some((multiplier, _)) = nearby.split_once('x') {
                if let Ok(explicit) = multiplier.parse::<usize>() {
                    count = count.max(explicit);
                }
            }
        }
    }
    count.clamp(1, 16)
}

pub fn is_supported_equipment(value: &str) -> bool {
    matches!(
        value,
        "bodyweight"
            | "dumbbell"
            | "kettlebell"
            | "band"
            | "medicine_ball"
            | "barbell"
            | "e_z_curl_bar"
            | "exercise_ball"
            | "foam_roll"
            | "cable"
            | "machine"
            | "pull_up_bar"
            | "v_bar"
            | "bench_or_box"
            | "dip_station"
            | "rack"
            | "wall"
            | "stable_support"
            | "leg_anchor"
    )
}

fn equipment_requirements(entry: &ExerciseCatalogEntry) -> Option<Vec<EquipmentRequirement>> {
    let base = entry.equipment.as_deref().and_then(normalize_equipment)?;
    let mut requirements = vec![EquipmentRequirement {
        kind: base,
        count: if base == "kettlebell"
            && DOUBLE_KETTLEBELL_EXERCISE_IDS.contains(&entry.id.as_str())
        {
            2
        } else {
            1
        },
    }];
    if PULL_UP_BAR_EXERCISE_IDS.contains(&entry.id.as_str()) {
        requirements.push(EquipmentRequirement {
            kind: "pull_up_bar",
            count: 1,
        });
    }
    if entry.id == "V-Bar_Pullup" {
        requirements.push(EquipmentRequirement {
            kind: "v_bar",
            count: 1,
        });
    }
    if BENCH_OR_BOX_EXERCISE_IDS.contains(&entry.id.as_str()) {
        requirements.push(EquipmentRequirement {
            kind: "bench_or_box",
            count: 1,
        });
    }
    if STABLE_SUPPORT_EXERCISE_IDS.contains(&entry.id.as_str()) {
        requirements.push(EquipmentRequirement {
            kind: "stable_support",
            count: 1,
        });
    }
    let supplemental = match entry.id.as_str() {
        "Dips_-_Triceps_Version" => Some(("dip_station", 1)),
        "Body_Tricep_Press" => Some(("barbell", 1)),
        "Close-Grip_Push-Up_off_of_a_Dumbbell" => Some(("dumbbell", 1)),
        "Crunch_-_Legs_On_Exercise_Ball" => Some(("exercise_ball", 1)),
        "Handstand_Push-Ups" => Some(("wall", 1)),
        "Natural_Glute_Ham_Raise" => Some(("leg_anchor", 1)),
        _ => None,
    };
    if let Some((kind, count)) = supplemental {
        requirements.push(EquipmentRequirement { kind, count });
    }
    if entry.id == "Body_Tricep_Press" {
        requirements.push(EquipmentRequirement {
            kind: "rack",
            count: 1,
        });
    }
    Some(requirements)
}

pub(crate) fn equipment_requirements_for_movement(
    movement_id: &str,
) -> Option<Vec<EquipmentRequirement>> {
    find(movement_id).and_then(equipment_requirements)
}

fn normalize_equipment(value: &str) -> Option<&'static str> {
    match value {
        "body only" => Some("bodyweight"),
        "dumbbell" => Some("dumbbell"),
        "kettlebells" => Some("kettlebell"),
        "bands" => Some("band"),
        "medicine ball" => Some("medicine_ball"),
        "barbell" => Some("barbell"),
        "e-z curl bar" => Some("e_z_curl_bar"),
        "exercise ball" => Some("exercise_ball"),
        "foam roll" => Some("foam_roll"),
        "cable" => Some("cable"),
        "machine" => Some("machine"),
        _ => None,
    }
}

pub fn display_name(id: &str) -> String {
    id.replace(['_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid_and_large() {
        validate().unwrap();
        assert_eq!(all().len(), 849);
        assert!(all().iter().all(|entry| entry.images.len() == 2));
        assert_eq!(
            all()
                .iter()
                .filter(|entry| !entry.instructions.is_empty())
                .count(),
            844
        );
        let goblet_squat = find("Goblet_Squat").unwrap();
        assert_eq!(goblet_squat.instructions.len(), 3);
        assert_eq!(
            goblet_squat.images,
            ["Goblet_Squat/0.jpg", "Goblet_Squat/1.jpg"]
        );
    }

    #[test]
    fn partner_required_exercises_are_removed_from_every_catalog_view() {
        let raw: Vec<ExerciseCatalogEntry> = serde_json::from_str(CATALOG_JSON).unwrap();
        let all_equipment = [
            "bodyweight",
            "dumbbell",
            "kettlebell",
            "band",
            "medicine_ball",
            "barbell",
            "e_z_curl_bar",
            "exercise_ball",
            "foam_roll",
            "cable",
            "machine",
            "pull_up_bar",
            "v_bar",
            "bench_or_box",
            "dip_station",
            "rack",
            "wall",
            "stable_support",
            "leg_anchor",
            "kettlebell",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let movements = movements_for_equipment(&all_equipment);

        assert_eq!(raw.len(), 873);
        assert_eq!(PARTNER_REQUIRED_EXERCISE_IDS.len(), 24);
        for id in PARTNER_REQUIRED_EXERCISE_IDS {
            assert!(raw.iter().any(|entry| entry.id == *id), "missing raw {id}");
            assert!(find(id).is_none(), "catalog exposed {id}");
            assert!(
                movements.iter().all(|movement| movement.id != *id),
                "movement pool exposed {id}"
            );
        }
    }

    #[test]
    fn exercises_with_solo_alternatives_remain_available() {
        for id in [
            "Backward_Medicine_Ball_Throw",
            "Floor_Glute-Ham_Raise",
            "Medicine_Ball_Chest_Pass",
            "Russian_Twist",
        ] {
            assert!(find(id).is_some(), "solo-capable exercise missing: {id}");
        }
    }

    #[test]
    fn movement_pool_policy_does_not_change_the_image_source_revision() {
        assert_eq!(CATALOG_REVISION, "b0eed061e1c832b3ed815fbaa4b45b3cdc14df49");
        assert_eq!(
            movement_pool_revision(),
            "b0eed061e1c832b3ed815fbaa4b45b3cdc14df49:equipment-v2"
        );
    }

    #[test]
    fn equipment_filter_always_includes_bodyweight_and_only_named_equipment() {
        let entries = candidates("one 12 kg dumbbell");
        assert!(entries
            .iter()
            .any(|entry| entry.equipment.as_deref() == Some("body only")));
        assert!(entries
            .iter()
            .any(|entry| entry.equipment.as_deref() == Some("dumbbell")));
        assert!(!entries
            .iter()
            .any(|entry| entry.equipment.as_deref() == Some("barbell")));
    }

    #[test]
    fn kettlebell_only_excludes_bodyweight_movements_that_require_props() {
        let entries = candidates("12kg kettlebell");
        let ids = entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<HashSet<_>>();

        assert!(ids.contains("Goblet_Squat"));
        assert!(ids.contains("Pushups"));
        for id in PULL_UP_BAR_EXERCISE_IDS
            .iter()
            .chain(BENCH_OR_BOX_EXERCISE_IDS)
            .chain(STABLE_SUPPORT_EXERCISE_IDS)
            .chain(
                [
                    "Dips_-_Triceps_Version",
                    "Body_Tricep_Press",
                    "Close-Grip_Push-Up_off_of_a_Dumbbell",
                    "Crunch_-_Legs_On_Exercise_Ball",
                    "Handstand_Push-Ups",
                    "Natural_Glute_Ham_Raise",
                ]
                .iter(),
            )
        {
            assert!(!ids.contains(id), "equipment filter exposed {id}");
        }
        for id in DOUBLE_KETTLEBELL_EXERCISE_IDS {
            assert!(!ids.contains(id), "single kettlebell exposed {id}");
        }
    }

    #[test]
    fn explicit_supplemental_equipment_restores_matching_movements() {
        assert!(candidates("pull-up bar")
            .iter()
            .any(|entry| entry.id == "Chin-Up"));
        assert!(!candidates("pull-up bar")
            .iter()
            .any(|entry| entry.id == "V-Bar_Pullup"));
        assert!(candidates("pull-up bar and v-bar")
            .iter()
            .any(|entry| entry.id == "V-Bar_Pullup"));
        assert!(candidates("flat bench")
            .iter()
            .any(|entry| entry.id == "Bench_Dips"));
        assert!(candidates("barbell and squat rack")
            .iter()
            .any(|entry| entry.id == "Body_Tricep_Press"));
        assert!(candidates("two 12kg kettlebells")
            .iter()
            .any(|entry| entry.id == "Double_Kettlebell_Jerk"));

        let complete = candidates(
            "two 12kg kettlebells, pull-up bar, v-bar, flat bench, dip station, \
             barbell, squat rack, dumbbell, exercise ball, wall, chair, and lat pulldown",
        );
        let ids = complete
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<HashSet<_>>();
        for id in PULL_UP_BAR_EXERCISE_IDS
            .iter()
            .chain(BENCH_OR_BOX_EXERCISE_IDS)
            .chain(STABLE_SUPPORT_EXERCISE_IDS)
            .chain(DOUBLE_KETTLEBELL_EXERCISE_IDS)
            .chain(
                [
                    "Dips_-_Triceps_Version",
                    "Body_Tricep_Press",
                    "Close-Grip_Push-Up_off_of_a_Dumbbell",
                    "Crunch_-_Legs_On_Exercise_Ball",
                    "Handstand_Push-Ups",
                    "Natural_Glute_Ham_Raise",
                ]
                .iter(),
            )
        {
            assert!(ids.contains(id), "complete inventory omitted {id}");
        }
    }

    #[test]
    fn equipment_quantity_parser_preserves_singular_and_multiple_kettlebells() {
        assert_eq!(
            locally_resolved_equipment("12kg kettlebell")
                .iter()
                .filter(|kind| kind.as_str() == "kettlebell")
                .count(),
            1
        );
        assert_eq!(
            locally_resolved_equipment("2x8kg kettlebells")
                .iter()
                .filter(|kind| kind.as_str() == "kettlebell")
                .count(),
            2
        );
    }

    #[test]
    fn serialized_entry_has_only_the_prompt_contract_fields() {
        let value = serde_json::to_value(&all()[0]).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(
            keys,
            HashSet::from([
                "id".into(),
                "force".into(),
                "mechanic".into(),
                "equipment".into(),
                "primaryMuscles".into(),
                "secondaryMuscles".into(),
                "category".into(),
            ])
        );
    }
}
