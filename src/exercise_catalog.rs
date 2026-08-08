use crate::models::{Movement, MovementSidedness, MovementStatus};
#[cfg(test)]
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

const CATALOG_JSON: &str = include_str!("../data/free-exercise-db.compact.json");
pub const CATALOG_REVISION: &str = "b0eed061e1c832b3ed815fbaa4b45b3cdc14df49";

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
}

pub fn all() -> &'static [ExerciseCatalogEntry] {
    static CATALOG: OnceLock<Vec<ExerciseCatalogEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).expect("bundled exercise catalog must be valid JSON")
    })
}

pub fn find(id: &str) -> Option<&'static ExerciseCatalogEntry> {
    all().iter().find(|entry| entry.id == id)
}

#[cfg(test)]
pub fn candidates(equipment_text: &str) -> Vec<ExerciseCatalogEntry> {
    candidates_for_equipment(&locally_resolved_equipment(equipment_text))
}

pub fn candidates_for_equipment(available: &[String]) -> Vec<ExerciseCatalogEntry> {
    let available = available.iter().map(String::as_str).collect::<HashSet<_>>();
    all()
        .iter()
        .filter(|entry| {
            entry
                .equipment
                .as_deref()
                .and_then(normalize_equipment)
                .is_some_and(|equipment| available.contains(equipment))
        })
        .cloned()
        .collect()
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
    }
    serde_json::from_str::<Vec<ExerciseCatalogEntry>>(CATALOG_JSON)
        .context("validating bundled exercise catalog")?;
    Ok(())
}

fn available_equipment(text: &str) -> HashSet<&'static str> {
    let text = text.to_lowercase();
    let mut available = HashSet::from(["bodyweight"]);
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
    ] {
        if aliases.iter().any(|alias| text.contains(alias)) {
            available.insert(kind);
        }
    }
    available
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
    )
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

fn display_name(id: &str) -> String {
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
        assert_eq!(all().len(), 873);
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
