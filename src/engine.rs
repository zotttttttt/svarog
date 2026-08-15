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
        let score = crate::exercise_catalog::find(&movement.id)
            .map(|entry| crate::recommender::archetype_score(config, entry))
            .unwrap_or(0);
        (
            prefer_mobility && !movement.mobility,
            movement.status == MovementStatus::Caution,
            std::cmp::Reverse(score),
            movement.estimated_seconds,
        )
    });

    let Some(movement) = candidates.into_iter().next() else {
        return Ok(None);
    };

    let reps = crate::recommender::adaptive_reps(store, &movement.id, movement.base_reps)?;

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
        side: None,
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

pub fn opportunity_allows(store: &Store, config: &Config) -> Result<bool> {
    if store.today_set_count()? >= config.preferences.max_daily_sets {
        return Ok(false);
    }
    let outcomes = store.recent_outcomes(5)?;
    let required = if outcomes.is_empty() {
        2
    } else {
        let adverse = outcomes
            .iter()
            .take(3)
            .filter(|item| item.status != "done" || item.actual_reps < item.prescribed_reps)
            .count();
        if outcomes.iter().any(|item| item.status == "pain") || adverse >= 2 {
            5
        } else if adverse == 1 {
            3
        } else {
            let streak = outcomes
                .iter()
                .take_while(|item| {
                    item.status == "done" && item.actual_reps >= item.prescribed_reps
                })
                .count();
            let threshold = if crate::archetypes::get(config.forge.archetype).stats.stamina >= 8 {
                3
            } else {
                5
            };
            if streak >= threshold {
                1
            } else {
                2
            }
        }
    };
    Ok(store.events_since_last_outcome()? >= required)
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
    fn opportunity_gate_starts_every_second_agent_run() {
        let store = test_store();
        let config = Config::default();
        store.insert_event(&event(90)).unwrap();

        assert!(!opportunity_allows(&store, &config).unwrap());

        store.insert_event(&event(90)).unwrap();
        assert!(opportunity_allows(&store, &config).unwrap());
    }

    #[test]
    fn opportunity_gate_backs_off_after_reduced_reps() {
        let store = test_store();
        let config = Config::default();
        let mut rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        rec.reps = 8;
        rec.id = Some(store.insert_recommendation(&rec).unwrap());
        store
            .record_set_with_reps(&rec, SetStatus::Done, 4)
            .unwrap();

        for _ in 0..2 {
            store.insert_event(&event(90)).unwrap();
        }
        assert!(!opportunity_allows(&store, &config).unwrap());
        store.insert_event(&event(90)).unwrap();
        assert!(opportunity_allows(&store, &config).unwrap());
    }

    #[test]
    fn pain_backoff_survives_a_later_compliant_forge() {
        let store = test_store();
        let config = Config::default();
        let mut painful = recommend(&store, &config, &event(90)).unwrap().unwrap();
        painful.id = Some(store.insert_recommendation(&painful).unwrap());
        store.record_set(&painful, SetStatus::Pain).unwrap();

        let mut completed = recommend(&store, &config, &event(90)).unwrap().unwrap();
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store.record_set(&completed, SetStatus::Done).unwrap();

        for _ in 0..4 {
            store.insert_event(&event(90)).unwrap();
        }
        assert!(!opportunity_allows(&store, &config).unwrap());
        store.insert_event(&event(90)).unwrap();
        assert!(opportunity_allows(&store, &config).unwrap());
    }

    #[test]
    fn high_stamina_archetype_accelerates_after_three_compliant_forges() {
        let store = test_store();
        let config = Config::default();
        for _ in 0..3 {
            let mut rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
            rec.id = Some(store.insert_recommendation(&rec).unwrap());
            store.record_set(&rec, SetStatus::Done).unwrap();
        }
        store.insert_event(&event(90)).unwrap();
        assert!(opportunity_allows(&store, &config).unwrap());
    }

    #[test]
    fn new_movement_uses_conservative_catalog_reps() {
        let store = test_store();
        let config = Config::default();

        let rec = recommend(&store, &config, &event(90)).unwrap().unwrap();
        let base_reps = store
            .movements()
            .unwrap()
            .into_iter()
            .find(|movement| movement.id == rec.movement_id)
            .unwrap()
            .base_reps;
        assert_eq!(rec.reps, base_reps);
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
            sidedness: crate::models::MovementSidedness::Bilateral,
        };
        let equipment_text = "12 kg kettlebell 2x8 kg dumbbells and a medical ball";

        assert!(equipment_matches(equipment_text, &movement));
        assert_eq!(
            choose_weight(equipment_text, &["kettlebell".to_string()]),
            Some(12.0)
        );
    }
}
