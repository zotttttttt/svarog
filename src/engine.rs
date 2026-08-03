use crate::config::Config;
use crate::models::{AgentEvent, AppStateKind, Movement, MovementStatus, Recommendation};
use crate::storage::{Store, MUSCLE_COOLDOWN_MINUTES};
use anyhow::Result;
use chrono::Utc;

pub fn recommend(
    store: &Store,
    config: &Config,
    event: &AgentEvent,
) -> Result<Option<Recommendation>> {
    let state = store.state()?;
    if matches!(
        state.kind,
        AppStateKind::Recommendation | AppStateKind::Active
    ) {
        return Ok(None);
    }

    if store.today_set_count()? >= config.preferences.max_daily_sets {
        return Ok(None);
    }
    if !event_frequency_allows(store, config)? {
        return Ok(None);
    }

    let movements = store.movements()?;
    let last_muscle = store.last_done_muscle()?;
    let intervention_count = store.intervention_count()?;
    let prefer_mobility = (intervention_count + 1) % 4 == 0;

    let mut candidates = Vec::new();
    for movement in movements {
        if movement.status == MovementStatus::Blocked {
            continue;
        }
        if Some(&movement.primary_muscle) == last_muscle.as_ref() {
            continue;
        }
        if movement.estimated_seconds + 15 > event.expected_duration_sec {
            continue;
        }
        if !equipment_matches(&config.profile.equipment_text, &movement) {
            continue;
        }
        if is_cautious_body_part(config, &movement) {
            continue;
        }
        if !store.muscle_recovered(&movement.primary_muscle, MUSCLE_COOLDOWN_MINUTES)? {
            continue;
        }
        candidates.push(movement);
    }

    candidates.sort_by_key(|movement| {
        (
            prefer_mobility && !movement.mobility,
            movement.status == MovementStatus::Caution,
            movement.estimated_seconds,
        )
    });

    let Some(movement) = candidates.into_iter().next() else {
        return Ok(None);
    };

    let skip_count = store.recent_skip_count(&movement.id)?;
    let done_count = store.done_count(&movement.id)?;
    let mut reps = apply_intensity(movement.base_reps, config.preferences.forge_intensity);

    if skip_count >= 3 {
        reps = reps.saturating_sub(2).max(1);
    } else if done_count >= 5 {
        reps += 1;
    }

    let weight_kg = choose_weight(&config.profile.equipment_text, &movement.equipment);

    Ok(Some(Recommendation {
        id: None,
        movement_id: movement.id,
        movement_name: movement.name,
        primary_muscle: movement.primary_muscle,
        muscles: movement.muscles,
        reps,
        weight_kg,
        estimated_seconds: movement.estimated_seconds,
        agent: event.agent,
        project: event.project.clone(),
        created_at: Utc::now(),
    }))
}

fn equipment_matches(equipment_text: &str, movement: &Movement) -> bool {
    let normalized = equipment_text.to_lowercase();
    movement.equipment.iter().any(|needed| {
        needed == "bodyweight"
            || normalized.contains(needed)
            || normalized.contains(&needed.replace('_', " "))
            || (needed == "band" && normalized.contains("resistance band"))
            || (needed == "medicine_ball"
                && (normalized.contains("medical ball") || normalized.contains("medicine ball")))
    })
}

fn choose_weight(equipment_text: &str, movement_equipment: &[String]) -> Option<f32> {
    let uses_weight = movement_equipment
        .iter()
        .any(|item| item == "dumbbell" || item == "kettlebell");
    if !uses_weight {
        return None;
    }
    extract_weights_kg(equipment_text)
        .into_iter()
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn extract_weights_kg(equipment_text: &str) -> Vec<f32> {
    let normalized = equipment_text.to_lowercase();
    let mut weights = Vec::new();
    let mut previous_number: Option<f32> = None;
    for token in normalized
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
    {
        if let Ok(value) = token.parse::<f32>() {
            previous_number = Some(value);
            continue;
        }
        if matches!(token, "kg" | "kgs" | "kilogram" | "kilograms") {
            if let Some(value) = previous_number.take() {
                weights.push(value);
            }
        }
    }
    weights
}

fn is_cautious_body_part(config: &Config, movement: &Movement) -> bool {
    movement.muscles.iter().any(|muscle| {
        config
            .profile
            .cautious_body_parts
            .iter()
            .chain(config.profile.injuries.iter())
            .any(|cautious| cautious.eq_ignore_ascii_case(muscle))
    })
}

fn event_frequency_allows(store: &Store, config: &Config) -> Result<bool> {
    let frequency = config.preferences.forge_frequency.max(1);
    Ok(store.event_count()? % frequency == 0)
}

fn apply_intensity(base_reps: u32, intensity: u32) -> u32 {
    base_reps + intensity.clamp(1, 5) - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{Agent, AgentEvent, SetStatus};
    use crate::storage::Store;
    use chrono::Utc;
    use tempfile::tempdir;

    fn test_store() -> Store {
        let dir = tempdir().unwrap().keep();
        let path = dir.join("test.sqlite3");
        let store = Store::open(&path).unwrap();
        store.seed_movements().unwrap();
        store
    }

    fn event(duration: u32) -> AgentEvent {
        AgentEvent {
            agent: Agent::Codex,
            event: "task_start".into(),
            expected_duration_sec: duration,
            project: Some("svarog".into()),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn rejects_movements_that_do_not_fit() {
        let store = test_store();
        let config = Config::default();
        let rec = recommend(&store, &config, &event(20)).unwrap();
        assert!(rec.is_none());
    }

    #[test]
    fn recommends_when_time_fits() {
        let store = test_store();
        let config = Config::default();
        let rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        assert!(rec.estimated_seconds <= 75);
    }

    #[test]
    fn pain_blocks_movement() {
        let store = test_store();
        let config = Config::default();
        let mut rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        let id = store.insert_recommendation(&rec).unwrap();
        rec.id = Some(id);
        store.record_set(&rec, SetStatus::Pain).unwrap();

        let next = recommend(&store, &config, &event(90)).unwrap();
        assert!(next.map(|candidate| candidate.movement_id) != Some(rec.movement_id));
    }

    #[test]
    fn stats_count_done_reps_and_breaks() {
        let store = test_store();
        let config = Config::default();
        let mut rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        let id = store.insert_recommendation(&rec).unwrap();
        rec.id = Some(id);
        store.record_set(&rec, SetStatus::Done).unwrap();
        store.record_set(&rec, SetStatus::Skipped).unwrap();

        let stats = store.stats_today().unwrap();
        assert_eq!(stats.0, 1);
        assert_eq!(stats.1, rec.reps);
        assert_eq!(stats.2, 1);
    }

    #[test]
    fn forge_frequency_blocks_non_matching_agent_runs() {
        let store = test_store();
        let mut config = Config::default();
        config.preferences.forge_frequency = 2;
        store.insert_event(&event(90)).unwrap();

        let next = recommend(&store, &config, &event(90)).unwrap();
        assert!(next.is_none());

        store.insert_event(&event(90)).unwrap();
        let next = recommend(&store, &config, &event(90)).unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn forge_intensity_adds_small_rep_adjustment() {
        let store = test_store();
        let mut config = Config::default();
        config.preferences.forge_intensity = 3;

        let rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        assert_eq!(rec.reps, 6);
    }

    #[test]
    fn equipment_helpers_use_raw_equipment_text() {
        let movement = Movement {
            id: "medicine_ball_hold".into(),
            name: "medicine ball hold".into(),
            primary_muscle: "core".into(),
            muscles: vec!["core".into()],
            equipment: vec!["medicine_ball".into()],
            base_reps: 1,
            estimated_seconds: 30,
            status: MovementStatus::Allowed,
            mobility: false,
        };
        let equipment_text = "12 kg kettlebell 2x8 kg dumbbells and a medical ball";

        assert!(equipment_matches(equipment_text, &movement));
        assert_eq!(
            choose_weight(equipment_text, &["kettlebell".to_string()]),
            Some(12.0)
        );
    }
}
