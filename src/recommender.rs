use crate::config::{Config, Paths, Profile, RecommenderBackend, UnitSystem};
use crate::exercise_catalog::{self, ExerciseCatalogEntry};
use crate::models::{
    AgentEvent, Movement, MovementSidedness, MovementStatus, Recommendation, RecommendationSide,
    RecommenderTokenProvider, RecommenderTokenUsage,
};
use crate::prompt_templates::PromptRenderer;
use crate::secrets;
use crate::storage::{SetSummary, Store, MUSCLE_COOLDOWN_MINUTES};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const QUEUE_TARGET: u32 = 10;
pub const QUEUE_LOW_WATER_MARK: u32 = 1;

#[derive(Debug)]
pub struct QueueGeneration {
    pub recommendations: Vec<Recommendation>,
    pub notice: Option<String>,
    pub source: QueueGenerationSource,
    pub llm_count: usize,
    pub local_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueGenerationSource {
    Codex,
    OpenAi,
    Local,
    LocalFallback,
    Hybrid,
}

#[derive(Debug, Clone, Serialize)]
struct RecommendationContext {
    profile: ContextProfile,
    preferences: ContextPreferences,
    expected_duration_sec: u32,
    today_stats: TodayStats,
    recent_sets: Vec<ContextSet>,
    app_state: ContextAppState,
    movements: Vec<ExerciseCatalogEntry>,
}

#[derive(Debug, Serialize)]
struct ExerciseProfileContext<'a> {
    profile: &'a Profile,
    preferences: ExerciseProfilePreferences,
    available_equipment: Vec<EquipmentCapability>,
}

#[derive(Debug, Serialize)]
struct ExerciseProfilePreferences {
    default_expected_duration_sec: u32,
    max_daily_sets: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ContextProfile {
    unit_system: UnitSystem,
    height_cm: Option<u32>,
    weight_kg: Option<f32>,
    age: Option<u32>,
    goals: Vec<String>,
    equipment_text: String,
    available_equipment: Vec<EquipmentCapability>,
    exercise_preferences: String,
    work_setup: String,
    one_hand_available: bool,
    two_hand_available: bool,
    cautious_body_parts: Vec<String>,
    injuries: Vec<String>,
    archetype: String,
    custom_archetype: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ContextPreferences {
    default_expected_duration_sec: u32,
    max_daily_sets: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ContextSet {
    movement_id: String,
    muscles: Vec<String>,
    status: String,
    reps: u32,
    prescribed_reps: u32,
    weight_kg: Option<f32>,
    side: Option<RecommendationSide>,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EquipmentCapability {
    kind: String,
    weights_kg: Vec<f32>,
    #[serde(default = "default_equipment_count")]
    count: u32,
}

fn default_equipment_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize)]
struct TodayStats {
    sets: u32,
    reps: u32,
    breaks: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ContextAppState {
    kind: String,
    cooldown_muscle: Option<String>,
    cooldown_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRecommendation {
    action: LlmAction,
    exercise_id: Option<String>,
    reps: Option<u32>,
    sets: Option<u32>,
    weight_text: Option<String>,
    duration_sec: Option<u32>,
    safety_notes: Option<String>,
    rationale: Option<String>,
    #[serde(default)]
    side: Option<RecommendationSide>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRecommendationBatch {
    recommendations: Vec<LlmRecommendation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmExerciseProfile {
    equipment: Vec<LlmEquipmentCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmEquipmentCapability {
    kind: String,
    weights_kg: Vec<f32>,
    #[serde(default = "default_equipment_count")]
    count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LlmAction {
    Recommend,
    NoRecommendation,
}

pub fn fill_recommendation_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
) -> Result<Option<String>> {
    let queued_recommendations = store.queued_recommendations()?;
    let queued = queued_recommendations.len() as u32;
    if queued > QUEUE_LOW_WATER_MARK {
        return Ok(None);
    }
    let generated = generate_recommendation_queue_with_existing(
        store,
        config,
        paths,
        QUEUE_TARGET,
        &queued_recommendations,
    )?;
    for rec in generated.recommendations {
        store.insert_queued_recommendation(&rec)?;
    }
    Ok(generated.notice)
}

pub fn generate_recommendation_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
    _requested: u32,
) -> Result<QueueGeneration> {
    generate_recommendation_queue_with_existing(store, config, paths, _requested, &[])
}

fn generate_recommendation_queue_with_existing(
    store: &Store,
    config: &Config,
    paths: &Paths,
    _requested: u32,
    existing: &[Recommendation],
) -> Result<QueueGeneration> {
    let needed = QUEUE_TARGET;
    let event = AgentEvent {
        agent: crate::models::Agent::Custom,
        event: "prefetch".to_string(),
        expected_duration_sec: config.preferences.default_expected_duration_sec,
        project: None,
        created_at: Utc::now(),
    };
    let (mut recommendations, notice, source, llm_count, local_count) = match config
        .recommender
        .backend
    {
        RecommenderBackend::Local => {
            let local = local_queue(store, config, &event, needed, existing)?;
            let local_count = local.len();
            (local, None, QueueGenerationSource::Local, 0, local_count)
        }
        RecommenderBackend::Codex
        | RecommenderBackend::OpenaiEnv
        | RecommenderBackend::OpenaiKeyring => {
            match llm_queue(store, config, paths, &event, needed, existing) {
                Ok(mut llm) => {
                    llm.truncate(needed as usize);
                    let llm_count = llm.len();
                    let local = if config.recommender.local_fallback && llm_count < needed as usize
                    {
                        local_queue(store, config, &event, needed - llm_count as u32, existing)?
                    } else {
                        Vec::new()
                    };
                    let local_count = local.len();
                    llm.extend(local);
                    let source = if local_count > 0 && llm_count > 0 {
                        QueueGenerationSource::Hybrid
                    } else if local_count > 0 {
                        QueueGenerationSource::LocalFallback
                    } else {
                        match config.recommender.backend {
                            RecommenderBackend::Codex => QueueGenerationSource::Codex,
                            RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
                                QueueGenerationSource::OpenAi
                            }
                            _ => unreachable!(),
                        }
                    };
                    let notice = hybrid_notice(config, llm_count, local_count);
                    (llm, notice, source, llm_count, local_count)
                }
                Err(err) if config.recommender.local_fallback => {
                    let fallback = local_queue(store, config, &event, needed, existing)?;
                    let local_count = fallback.len();
                    (
                        fallback,
                        fallback_notice(config, &err),
                        QueueGenerationSource::LocalFallback,
                        0,
                        local_count,
                    )
                }
                Err(err) => return Err(err),
            }
        }
    };

    recommendations = schedule_equipment_batch(store.movements()?, recommendations);
    recommendations.truncate(needed as usize);
    Ok(QueueGeneration {
        recommendations,
        notice,
        source,
        llm_count,
        local_count,
    })
}

fn hybrid_notice(config: &Config, llm_count: usize, local_count: usize) -> Option<String> {
    if !config.recommender.show_llm_failures || local_count == 0 {
        return None;
    }
    let backend = match config.recommender.backend {
        RecommenderBackend::Codex => "Codex",
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => "OpenAI",
        _ => "Recommender",
    };
    if llm_count == 0 {
        Some(format!("{backend} did not suggest any safe forges."))
    } else {
        Some(format!(
            "{backend} suggested {llm_count}; Svarog filled {local_count} locally."
        ))
    }
}

fn fallback_notice(config: &Config, error: &anyhow::Error) -> Option<String> {
    if !config.recommender.show_llm_failures {
        return None;
    }
    let backend = match config.recommender.backend {
        RecommenderBackend::Codex => "Codex",
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => "OpenAI",
        _ => "Recommender",
    };
    let details = format!("{error:#}").to_lowercase();
    if details.contains("parsing codex json")
        || details.contains("parsing openai recommendation queue")
        || details.contains("invalid recommendation json")
        || details.contains("no json object found")
    {
        Some(format!("{backend} returned an invalid response."))
    } else if details.contains("timed out") {
        Some(format!("{backend} timed out."))
    } else {
        Some(format!("{backend} was unavailable."))
    }
}

fn local_queue(
    store: &Store,
    config: &Config,
    event: &AgentEvent,
    needed: u32,
    existing: &[Recommendation],
) -> Result<Vec<Recommendation>> {
    let mut used_muscles = existing
        .iter()
        .map(|recommendation| recommendation.primary_muscle.clone())
        .collect::<Vec<_>>();
    let used_movements = existing
        .iter()
        .map(|recommendation| recommendation.movement_id.as_str())
        .collect::<Vec<_>>();
    let mut movements = store
        .movements()?
        .into_iter()
        .map(|movement| {
            let recovered =
                store.muscle_recovered(&movement.primary_muscle, MUSCLE_COOLDOWN_MINUTES)?;
            Ok((movement, recovered))
        })
        .collect::<Result<Vec<_>>>()?;
    movements.sort_by_key(|(movement, recovered)| {
        let score = exercise_catalog::find(&movement.id)
            .map(|entry| archetype_score(config, entry))
            .unwrap_or(0);
        (
            !*recovered,
            movement.status == MovementStatus::Caution,
            std::cmp::Reverse(score),
            !movement.mobility,
            movement.estimated_seconds,
        )
    });

    let mut recommendations = Vec::new();
    for (movement, _) in movements {
        if movement.status == MovementStatus::Blocked {
            continue;
        }
        if used_movements.contains(&movement.id.as_str()) {
            continue;
        }
        if used_muscles.contains(&movement.primary_muscle) {
            continue;
        }
        if movement.estimated_seconds + 15 > event.expected_duration_sec {
            continue;
        }
        if conflicts_with_limitations(config, &movement.primary_muscle, &movement.muscles) {
            continue;
        }
        used_muscles.push(movement.primary_muscle.clone());
        let reps = adaptive_reps(store, &movement.id, movement.base_reps)?;
        recommendations.push(Recommendation {
            id: None,
            movement_id: movement.id,
            movement_name: movement.name,
            primary_muscle: movement.primary_muscle,
            muscles: movement.muscles,
            reps,
            weight_kg: None,
            estimated_seconds: movement.estimated_seconds,
            agent: event.agent,
            project: event.project.clone(),
            side: None,
            created_at: Utc::now(),
        });
        if recommendations.len() >= needed as usize {
            break;
        }
    }
    Ok(recommendations)
}

pub(crate) fn archetype_score(config: &Config, entry: &ExerciseCatalogEntry) -> u32 {
    let archetype = crate::archetypes::get(config.forge.archetype);
    let category = entry.category.as_str();
    let mut score = 0;
    if archetype.preferred_categories.contains(&category) {
        score += 30;
    }
    if entry
        .force
        .as_deref()
        .is_some_and(|value| archetype.preferred_forces.contains(&value))
    {
        score += 12;
    }
    if entry
        .mechanic
        .as_deref()
        .is_some_and(|value| archetype.preferred_mechanics.contains(&value))
    {
        score += 10;
    }
    score += entry
        .primary_muscles
        .iter()
        .filter(|muscle| archetype.preferred_muscles.contains(&muscle.as_str()))
        .count() as u32
        * 8;
    if entry.category == "stretching" {
        score += u32::from(archetype.stats.mobility);
    }
    if entry.category == "cardio" {
        score += u32::from(archetype.stats.cardio);
    }
    if entry.category == "strength" {
        score += u32::from(archetype.stats.strength + archetype.stats.muscle) / 2;
    }
    score
}

pub fn adaptive_reps(store: &Store, movement_id: &str, base: u32) -> Result<u32> {
    let outcomes = store.recent_movement_outcomes(movement_id, 5)?;
    let Some(latest) = outcomes.first() else {
        return Ok(base.clamp(1, 20));
    };
    let current = latest.prescribed_reps.clamp(1, 20);
    if latest.status == "done" && latest.actual_reps < latest.prescribed_reps {
        return Ok(latest.actual_reps.clamp(1, 20));
    }
    let adverse = outcomes
        .iter()
        .take(3)
        .filter(|item| item.status != "done")
        .count();
    if adverse >= 2 {
        return Ok(current.saturating_sub(1).max(1));
    }
    if adverse == 1 {
        return Ok(current);
    }
    let increased_twice = outcomes.iter().take(2).count() == 2
        && outcomes
            .iter()
            .take(2)
            .all(|item| item.status == "done" && item.actual_reps > item.prescribed_reps);
    let compliant_five = outcomes.iter().take(5).count() == 5
        && outcomes
            .iter()
            .take(5)
            .all(|item| item.status == "done" && item.actual_reps >= item.prescribed_reps);
    Ok(if increased_twice || compliant_five {
        current.saturating_add(1).min(20)
    } else {
        current
    })
}

fn llm_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
    event: &AgentEvent,
    needed: u32,
    existing: &[Recommendation],
) -> Result<Vec<Recommendation>> {
    let context = build_context(store, config, event)?;
    let prompt = PromptRenderer::new(&paths.config_dir).recommendation_queue(&context, needed)?;
    let batch = match config.recommender.backend {
        RecommenderBackend::Codex => call_codex_queue(store, config, &prompt, needed),
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
            call_openai_queue(store, config, paths, &prompt, needed)
        }
        RecommenderBackend::Local => unreachable!(),
    }?;
    let movements = store.movements()?;
    let mut recommendations: Vec<Recommendation> = existing.to_vec();
    for candidate in batch.recommendations {
        let Ok(Some(rec)) = validate_candidate(config, event, candidate, &movements) else {
            continue;
        };
        let repeats_movement = recommendations.iter().any(|existing| {
            existing.movement_id == rec.movement_id && !is_opposite_side_pair(existing, &rec)
        });
        let repeats_muscle = recommendations.iter().any(|existing| {
            existing.primary_muscle == rec.primary_muscle
                && !recommendations
                    .last()
                    .is_some_and(|last| is_opposite_side_pair(last, &rec))
        });
        if !repeats_movement && !repeats_muscle {
            recommendations.push(rec);
        }
        if recommendations.len() >= existing.len() + needed as usize {
            break;
        }
    }
    Ok(recommendations.into_iter().skip(existing.len()).collect())
}

fn is_opposite_side_pair(left: &Recommendation, right: &Recommendation) -> bool {
    left.movement_id == right.movement_id
        && matches!(
            (left.side, right.side),
            (
                Some(RecommendationSide::Left),
                Some(RecommendationSide::Right)
            ) | (
                Some(RecommendationSide::Right),
                Some(RecommendationSide::Left)
            )
        )
}

fn schedule_equipment_batch(
    movements: Vec<Movement>,
    recommendations: Vec<Recommendation>,
) -> Vec<Recommendation> {
    let movement_by_id = movements
        .into_iter()
        .map(|movement| (movement.id.clone(), movement))
        .collect::<std::collections::HashMap<_, _>>();
    let is_equipment = |recommendation: &Recommendation| {
        movement_by_id
            .get(&recommendation.movement_id)
            .is_some_and(|movement| {
                movement
                    .equipment
                    .iter()
                    .any(|equipment| equipment != "bodyweight")
            })
    };
    let equipment_count = recommendations
        .iter()
        .filter(|rec| is_equipment(rec))
        .count();
    if equipment_count < 5 {
        return recommendations;
    }

    let mut equipment = recommendations
        .iter()
        .filter(|rec| is_equipment(rec))
        .cloned()
        .collect::<Vec<_>>();
    let mut other = recommendations
        .iter()
        .filter(|rec| !is_equipment(rec))
        .cloned()
        .collect::<Vec<_>>();
    let unilateral = equipment.iter().any(|rec| {
        movement_by_id
            .get(&rec.movement_id)
            .is_some_and(|movement| movement.sidedness == MovementSidedness::Unilateral)
    });

    if unilateral {
        equipment.sort_by_key(|rec| {
            (
                rec.movement_id.clone(),
                matches!(rec.side, Some(RecommendationSide::Left)),
            )
        });
        let equipment = equipment.into_iter().take(8).collect::<Vec<_>>();
        other.truncate(recommendations.len().saturating_sub(equipment.len()));
        other.extend(equipment);
        other
    } else {
        let mut scheduled = Vec::with_capacity(recommendations.len());
        let mut equipment_iter = equipment.into_iter();
        let mut other_iter = other.into_iter();
        for index in 0..recommendations.len() {
            let item = if index % 2 == 1 {
                equipment_iter.next().or_else(|| other_iter.next())
            } else {
                other_iter.next().or_else(|| equipment_iter.next())
            };
            if let Some(item) = item {
                scheduled.push(item);
            }
        }
        scheduled
    }
}

pub fn initial_exercise_profile(
    store: &Store,
    config: &Config,
    paths: &Paths,
) -> (Vec<Movement>, Option<String>) {
    if config.recommender.backend == RecommenderBackend::Local {
        let (movements, equipment) = local_resolved_movements(config);
        let _ = store.save_exercise_filter(&equipment);
        return (movements, None);
    }

    let result = PromptRenderer::new(&paths.config_dir)
        .exercise_profile(&exercise_profile_context(config))
        .and_then(|prompt| match config.recommender.backend {
            RecommenderBackend::Codex => call_codex_exercise_profile(store, config, &prompt),
            RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
                call_openai_exercise_profile(store, config, paths, &prompt)
            }
            RecommenderBackend::Local => unreachable!(),
        })
        .and_then(validate_exercise_profile)
        .and_then(|(movements, equipment)| {
            store.save_exercise_filter(&equipment)?;
            Ok(movements)
        });

    match result {
        Ok(movements) if !movements.is_empty() => (movements, None),
        Ok(_) => {
            let (movements, equipment) = local_resolved_movements(config);
            let _ = store.save_exercise_filter(&equipment);
            (movements, None)
        }
        Err(err) => (
            {
                let (movements, equipment) = local_resolved_movements(config);
                let _ = store.save_exercise_filter(&equipment);
                movements
            },
            config
                .recommender
                .show_llm_failures
                .then(|| format!("Using conservative local equipment filtering: {err}")),
        ),
    }
}

fn exercise_profile_context(config: &Config) -> ExerciseProfileContext<'_> {
    ExerciseProfileContext {
        profile: &config.profile,
        preferences: ExerciseProfilePreferences {
            default_expected_duration_sec: config.preferences.default_expected_duration_sec,
            max_daily_sets: config.preferences.max_daily_sets,
        },
        available_equipment: normalize_equipment(&config.profile.equipment_text),
    }
}

pub(crate) fn normalize_equipment(text: &str) -> Vec<EquipmentCapability> {
    let normalized = text.to_lowercase();
    let weights = extract_weight_values_kg(&normalized);
    let mut counts = std::collections::BTreeMap::<String, u32>::new();
    for kind in exercise_catalog::locally_resolved_equipment(text) {
        *counts.entry(kind).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| EquipmentCapability {
            weights_kg: if matches!(kind.as_str(), "kettlebell" | "dumbbell" | "barbell") {
                weights.clone()
            } else {
                Vec::new()
            },
            kind,
            count,
        })
        .collect()
}

fn extract_weight_values_kg(text: &str) -> Vec<f32> {
    let mut weights = Vec::new();
    let mut previous_number = None;
    for token in text
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
    {
        if let Ok(value) = token.parse::<f32>() {
            previous_number = Some(value);
        } else if let Some((_, value)) = token.rsplit_once('x') {
            if let Ok(value) = value.parse::<f32>() {
                previous_number = Some(value);
            }
        } else if matches!(token, "kg" | "kgs" | "kilogram" | "kilograms") {
            if let Some(value) = previous_number.take() {
                weights.push(value);
            }
        }
    }
    weights
}

fn build_context(
    store: &Store,
    config: &Config,
    event: &AgentEvent,
) -> Result<RecommendationContext> {
    let (sets, reps, breaks) = store.stats_today()?;
    let state = store.state()?;
    let profile = &config.profile;
    let preferences = &config.preferences;
    let available_equipment = store
        .exercise_filter::<Vec<EquipmentCapability>>()?
        .unwrap_or_else(|| normalize_equipment(&profile.equipment_text));
    Ok(RecommendationContext {
        profile: ContextProfile {
            unit_system: profile.unit_system,
            height_cm: profile.height_cm,
            weight_kg: profile.weight_kg,
            age: profile.age,
            goals: profile.goals.clone(),
            equipment_text: profile.equipment_text.clone(),
            available_equipment,
            exercise_preferences: profile.exercise_preferences.clone(),
            work_setup: profile.work_setup.clone(),
            one_hand_available: profile.one_hand_available,
            two_hand_available: profile.two_hand_available,
            cautious_body_parts: profile.cautious_body_parts.clone(),
            injuries: profile.injuries.clone(),
            archetype: config.forge.archetype.as_str().to_string(),
            custom_archetype: config.forge.custom_archetype.clone(),
        },
        preferences: ContextPreferences {
            default_expected_duration_sec: preferences.default_expected_duration_sec,
            max_daily_sets: preferences.max_daily_sets,
        },
        expected_duration_sec: event.expected_duration_sec,
        today_stats: TodayStats { sets, reps, breaks },
        recent_sets: store
            .completed_sets_today_and_yesterday()?
            .into_iter()
            .map(ContextSet::from)
            .collect(),
        app_state: ContextAppState {
            kind: state.kind.as_str().to_string(),
            cooldown_muscle: state.cooldown_muscle,
            cooldown_until: state.cooldown_until.map(|dt| dt.to_rfc3339()),
        },
        movements: exercise_catalog::entries_for_movements(&store.movements()?),
    })
}

impl From<SetSummary> for ContextSet {
    fn from(value: SetSummary) -> Self {
        Self {
            movement_id: value.movement_id,
            muscles: value.muscles,
            status: value.status,
            reps: value.reps,
            prescribed_reps: value.prescribed_reps,
            weight_kg: value.weight_kg,
            side: value.side,
            created_at: value.created_at,
        }
    }
}

fn call_codex_queue(
    store: &Store,
    config: &Config,
    prompt: &str,
    needed: u32,
) -> Result<LlmRecommendationBatch> {
    call_codex_json(store, config, prompt, &recommendation_queue_schema(needed))
}

fn call_codex_exercise_profile(
    store: &Store,
    config: &Config,
    prompt: &str,
) -> Result<LlmExerciseProfile> {
    call_codex_json(store, config, prompt, &exercise_profile_schema())
}

fn call_codex_json<T>(
    store: &Store,
    config: &Config,
    prompt: &str,
    schema: &serde_json::Value,
) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut schema_file = tempfile::NamedTempFile::new().context("creating Codex output schema")?;
    serde_json::to_writer(&mut schema_file, schema).context("writing Codex output schema")?;
    schema_file
        .flush()
        .context("flushing Codex output schema")?;
    let deadline = Instant::now() + Duration::from_millis(config.recommender.timeout_ms);
    let configured_model = config.recommender.codex.model.trim();
    let model = (!configured_model.eq_ignore_ascii_case("inherit") && !configured_model.is_empty())
        .then_some(configured_model);

    match call_codex_json_attempt(
        store,
        config,
        prompt,
        schema_file.path(),
        model,
        deadline,
        None,
    )? {
        CodexAttempt::Completed(value) => Ok(value),
        CodexAttempt::EarlyFailure(first_error) if model.is_some() => {
            match call_codex_json_attempt(
                store,
                config,
                prompt,
                schema_file.path(),
                None,
                deadline,
                None,
            )
            .with_context(|| format!("{first_error}; inherited Codex model retry failed"))?
            {
                CodexAttempt::Completed(value) => Ok(value),
                CodexAttempt::EarlyFailure(second_error) => {
                    bail!("{first_error}; inherited Codex model also failed: {second_error}")
                }
            }
        }
        CodexAttempt::EarlyFailure(error) => bail!(error),
    }
}

pub(crate) fn call_codex_json_for_model<T>(
    store: &Store,
    config: &Config,
    prompt: &str,
    schema: &serde_json::Value,
    model: &str,
    cancel: &AtomicBool,
) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut schema_file = tempfile::NamedTempFile::new().context("creating Codex output schema")?;
    serde_json::to_writer(&mut schema_file, schema).context("writing Codex output schema")?;
    schema_file
        .flush()
        .context("flushing Codex output schema")?;
    let deadline = Instant::now() + Duration::from_millis(config.recommender.timeout_ms);
    match call_codex_json_attempt(
        store,
        config,
        prompt,
        schema_file.path(),
        Some(model),
        deadline,
        Some(cancel),
    )? {
        CodexAttempt::Completed(value) => Ok(value),
        CodexAttempt::EarlyFailure(error) => bail!(error),
    }
}

enum CodexAttempt<T> {
    Completed(T),
    EarlyFailure(String),
}

fn call_codex_json_attempt<T>(
    store: &Store,
    config: &Config,
    prompt: &str,
    schema_path: &Path,
    model: Option<&str>,
    deadline: Instant,
    cancel: Option<&AtomicBool>,
) -> Result<CodexAttempt<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if Instant::now() >= deadline {
        bail!("codex recommender timed out");
    }
    let mut command = Command::new(&config.recommender.codex.command);
    command.process_group(0);
    command.args(codex_args_without_output_schema(
        &config.recommender.codex.args,
    ));
    if !config
        .recommender
        .codex
        .args
        .iter()
        .any(|argument| argument == "--json")
    {
        command.arg("--json");
    }
    command.arg("--output-schema").arg(schema_path);
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    let mut child = command
        .arg(prompt)
        .env("SVAROG_RECOMMENDER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", config.recommender.codex.command))?;

    let mut stdout = child.stdout.take().context("capturing Codex stdout")?;
    let mut stderr = child.stderr.take().context("capturing Codex stderr")?;
    let stdout_reader = thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).map(|_| output)
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        stderr.read_to_string(&mut output).map(|_| output)
    });

    loop {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            kill_and_reap(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            bail!("nutrition parsing cancelled");
        }
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader
                .join()
                .map_err(|_| anyhow!("Codex stdout reader panicked"))??;
            let stderr = stderr_reader
                .join()
                .map_err(|_| anyhow!("Codex stderr reader panicked"))??;
            let parsed = parse_codex_jsonl(&stdout);
            if let Ok((_, Some(usage))) = &parsed {
                let _ = store.record_recommender_token_usage(
                    RecommenderTokenProvider::Codex,
                    usage,
                    Utc::now(),
                );
            }
            if !status.success() {
                let error = codex_process_error(&stderr, status.code());
                if !codex_turn_completed(&stdout) {
                    return Ok(CodexAttempt::EarlyFailure(error));
                }
                bail!(error);
            }
            let (message, _) = parsed?;
            return parse_llm_json(&message)
                .context("parsing codex JSON")
                .map(CodexAttempt::Completed);
        }
        if Instant::now() >= deadline {
            kill_and_reap(&mut child);
            bail!("codex recommender timed out");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn kill_and_reap(child: &mut std::process::Child) {
    let process_group = -(child.id() as i32);
    // SAFETY: the child was placed in its own process group immediately before spawn.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn codex_args_without_output_schema(args: &[String]) -> Vec<&str> {
    let mut retained = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--output-schema" {
            index += usize::from(index + 1 < args.len()) + 1;
        } else if args[index].starts_with("--output-schema=") {
            index += 1;
        } else {
            retained.push(args[index].as_str());
            index += 1;
        }
    }
    retained
}

fn codex_process_error(stderr: &str, code: Option<i32>) -> String {
    let detail = stderr.trim();
    if detail.is_empty() {
        match code {
            Some(code) => format!("codex recommender failed with exit code {code}"),
            None => "codex recommender failed".to_string(),
        }
    } else {
        format!("codex recommender failed: {detail}")
    }
}

fn codex_turn_completed(output: &str) -> bool {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|event| event["type"].as_str() == Some("turn.completed"))
}

fn parse_codex_jsonl(output: &str) -> Result<(String, Option<RecommenderTokenUsage>)> {
    let mut final_message = None;
    let mut usage = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let event: serde_json::Value =
            serde_json::from_str(line).context("parsing Codex JSONL event")?;
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("item.completed") if event["item"]["type"].as_str() == Some("agent_message") => {
                if let Some(text) = event["item"]["text"].as_str() {
                    final_message = Some(text.to_string());
                }
            }
            Some("turn.completed") => {
                usage = event
                    .get("usage")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .context("parsing Codex token usage")?;
            }
            _ => {}
        }
    }
    Ok((
        final_message.context("Codex JSONL did not contain a completed agent message")?,
        usage,
    ))
}

fn call_openai_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
    prompt: &str,
    needed: u32,
) -> Result<LlmRecommendationBatch> {
    let body = openai_queue_request_body(config, prompt, needed)?;
    call_openai_json(store, config, paths, body).context("parsing OpenAI recommendation queue")
}

fn call_openai_exercise_profile(
    store: &Store,
    config: &Config,
    paths: &Paths,
    prompt: &str,
) -> Result<LlmExerciseProfile> {
    let body = openai_exercise_profile_request_body(config, prompt)?;
    call_openai_json(store, config, paths, body).context("parsing OpenAI exercise profile")
}

pub(crate) fn call_openai_json<T>(
    store: &Store,
    config: &Config,
    paths: &Paths,
    body: serde_json::Value,
) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let api_key = match config.recommender.backend {
        RecommenderBackend::OpenaiEnv => std::env::var(&config.recommender.openai.api_key_env)
            .map(zeroize::Zeroizing::new)
            .with_context(|| format!("missing {}", config.recommender.openai.api_key_env))?,
        RecommenderBackend::OpenaiKeyring => secrets::openai_api_key(paths)?
            .filter(|key| !key.trim().is_empty())
            .context("no OpenAI API key is saved in Svarog Settings")?,
        _ => unreachable!(),
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(config.recommender.timeout_ms))
        .build()?;
    let response = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(api_key.as_str())
        .json(&body)
        .send()
        .context("calling OpenAI Responses API")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!(openai_api_error_message(status, &body));
    }
    let response: serde_json::Value = response.json().context("parsing OpenAI response JSON")?;
    if let Some(usage) = parse_openai_usage(&response)? {
        let _ = store.record_recommender_token_usage(
            RecommenderTokenProvider::OpenAi,
            &usage,
            Utc::now(),
        );
    }
    let text = response
        .get("output_text")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| find_first_text(&response));
    let Some(text) = text else {
        bail!("OpenAI response did not contain output text");
    };
    parse_llm_json(&text)
}

pub(crate) fn call_openai_json_cancellable<T>(
    store: &Store,
    config: &Config,
    paths: &Paths,
    body: serde_json::Value,
    cancel: Arc<AtomicBool>,
) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let api_key = match config.recommender.backend {
        RecommenderBackend::OpenaiEnv => std::env::var(&config.recommender.openai.api_key_env)
            .map(zeroize::Zeroizing::new)
            .with_context(|| format!("missing {}", config.recommender.openai.api_key_env))?,
        RecommenderBackend::OpenaiKeyring => secrets::openai_api_key(paths)?
            .filter(|key| !key.trim().is_empty())
            .context("no OpenAI API key is saved in Svarog Settings")?,
        _ => unreachable!(),
    };
    if cancel.load(Ordering::Acquire) {
        bail!("nutrition parsing cancelled");
    }
    let timeout = Duration::from_millis(config.recommender.timeout_ms);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting cancellable OpenAI runtime")?;
    let response: serde_json::Value = runtime.block_on(async {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        let request = async {
            let response = client
                .post("https://api.openai.com/v1/responses")
                .bearer_auth(api_key.as_str())
                .json(&body)
                .send()
                .await
                .context("calling OpenAI Responses API")?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                bail!(openai_api_error_message(status, &body));
            }
            response
                .json()
                .await
                .context("parsing OpenAI response JSON")
        };
        tokio::select! {
            response = request => response,
            _ = wait_for_cancellation(cancel.clone()) => {
                bail!("nutrition parsing cancelled");
            }
        }
    })?;
    if cancel.load(Ordering::Acquire) {
        bail!("nutrition parsing cancelled");
    }
    if let Some(usage) = parse_openai_usage(&response)? {
        let _ = store.record_recommender_token_usage(
            RecommenderTokenProvider::OpenAi,
            &usage,
            Utc::now(),
        );
    }
    let text = response
        .get("output_text")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| find_first_text(&response))
        .context("OpenAI response did not contain output text")?;
    parse_llm_json(&text)
}

async fn wait_for_cancellation(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn openai_api_error_message(status: reqwest::StatusCode, body: &str) -> String {
    let error = serde_json::from_str::<serde_json::Value>(body).ok();
    let detail = error
        .as_ref()
        .and_then(|value| value.pointer("/error/message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let code = error
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|code| !code.is_empty());
    let status = match code {
        Some(code) => format!("{status} ({code})"),
        None => status.to_string(),
    };
    if detail.is_empty() {
        format!("OpenAI Responses API returned {status}")
    } else {
        format!("OpenAI Responses API returned {status}: {detail}")
    }
}

fn parse_openai_usage(response: &serde_json::Value) -> Result<Option<RecommenderTokenUsage>> {
    let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) else {
        return Ok(None);
    };
    Ok(Some(RecommenderTokenUsage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .context("OpenAI usage missing input_tokens")?,
        cached_input_tokens: usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64)
            .context("OpenAI usage missing output_tokens")?,
        reasoning_output_tokens: usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    }))
}

fn openai_queue_request_body(
    config: &Config,
    prompt: &str,
    needed: u32,
) -> Result<serde_json::Value> {
    Ok(json!({
        "model": config.recommender.openai.model,
        "reasoning": {
            "effort": config.recommender.openai.reasoning_effort
        },
        "input": [
            {
                "role": "user",
                "content": prompt
            }
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "svarog_recommendation_queue",
                "strict": true,
                "schema": recommendation_queue_schema(needed)
            }
        }
    }))
}

fn recommendation_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "action", "exercise_id", "reps", "sets", "weight_text", "duration_sec",
            "safety_notes", "rationale"
            , "side"
        ],
        "properties": {
            "action": { "type": "string", "enum": ["recommend", "no_recommendation"] },
            "exercise_id": { "type": ["string", "null"] },
            "reps": { "type": ["integer", "null"], "minimum": 1, "maximum": 30 },
            "sets": { "type": ["integer", "null"], "minimum": 1, "maximum": 3 },
            "weight_text": { "type": ["string", "null"] },
            "duration_sec": { "type": ["integer", "null"], "minimum": 1, "maximum": 600 },
            "safety_notes": { "type": ["string", "null"] },
            "rationale": { "type": ["string", "null"] }
            , "side": { "type": ["string", "null"], "enum": ["left", "right", "bilateral", null] }
        }
    })
}

fn recommendation_queue_schema(needed: u32) -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["recommendations"],
        "properties": {
            "recommendations": {
                "type": "array",
                "minItems": needed,
                "maxItems": needed,
                "items": recommendation_schema()
            }
        }
    })
}

fn openai_exercise_profile_request_body(
    config: &Config,
    prompt: &str,
) -> Result<serde_json::Value> {
    Ok(json!({
        "model": config.recommender.openai.model,
        "reasoning": {
            "effort": config.recommender.openai.reasoning_effort
        },
        "input": [
            {
                "role": "user",
                "content": prompt
            }
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "svarog_exercise_profile",
                "strict": true,
                "schema": exercise_profile_schema()
            }
        }
    }))
}

fn exercise_profile_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["equipment"],
        "properties": {
            "equipment": {
                "type": "array",
                "minItems": 0,
                "maxItems": 20,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind", "weights_kg", "count"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "bodyweight", "dumbbell", "kettlebell", "band",
                                "medicine_ball", "barbell", "e_z_curl_bar",
                                "exercise_ball", "foam_roll", "cable", "machine",
                                "pull_up_bar", "v_bar", "bench_or_box", "dip_station",
                                "rack", "wall", "stable_support", "leg_anchor"
                            ]
                        },
                        "weights_kg": {
                            "type": "array",
                            "items": { "type": "number", "minimum": 0.25, "maximum": 300 },
                            "maxItems": 16
                        },
                        "count": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 16
                        }
                    }
                }
            }
        }
    })
}

fn parse_llm_json<T>(text: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let trimmed = text.trim();
    if let Ok(parsed) = serde_json::from_str(trimmed) {
        return Ok(parsed);
    }
    let start = trimmed
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object found"))?;
    let end = trimmed
        .rfind('}')
        .ok_or_else(|| anyhow!("no JSON object found"))?;
    serde_json::from_str(&trimmed[start..=end]).context("invalid recommendation JSON")
}

fn validate_exercise_profile(
    profile: LlmExerciseProfile,
) -> Result<(Vec<Movement>, Vec<EquipmentCapability>)> {
    let mut seen = std::collections::HashSet::new();
    let mut equipment = vec![EquipmentCapability {
        kind: "bodyweight".to_string(),
        weights_kg: Vec::new(),
        count: 1,
    }];
    for capability in profile.equipment {
        if !exercise_catalog::is_supported_equipment(&capability.kind) {
            bail!(
                "LLM equipment resolver returned unsupported kind {}",
                capability.kind
            );
        }
        if capability
            .weights_kg
            .iter()
            .any(|weight| !weight.is_finite() || !(0.25..=300.0).contains(weight))
        {
            bail!("LLM equipment resolver returned an invalid weight");
        }
        if !(1..=16).contains(&capability.count) {
            bail!("LLM equipment resolver returned an invalid equipment count");
        }
        if seen.insert(capability.kind.clone()) && capability.kind != "bodyweight" {
            equipment.push(EquipmentCapability {
                kind: capability.kind,
                weights_kg: capability.weights_kg,
                count: capability.count,
            });
        }
    }
    let kinds = equipment
        .iter()
        .flat_map(|capability| {
            std::iter::repeat_n(capability.kind.clone(), capability.count as usize)
        })
        .collect::<Vec<_>>();
    Ok((exercise_catalog::movements_for_equipment(&kinds), equipment))
}

fn local_resolved_movements(config: &Config) -> (Vec<Movement>, Vec<EquipmentCapability>) {
    let kinds = exercise_catalog::locally_resolved_equipment(&config.profile.equipment_text);
    (
        exercise_catalog::movements_for_equipment(&kinds),
        normalize_equipment(&config.profile.equipment_text),
    )
}

fn validate_candidate(
    _config: &Config,
    event: &AgentEvent,
    candidate: LlmRecommendation,
    movements: &[Movement],
) -> Result<Option<Recommendation>> {
    if candidate.action == LlmAction::NoRecommendation {
        return Ok(None);
    }

    let exercise_id = required(candidate.exercise_id, "exercise_id")?;
    let reps = required(candidate.reps, "reps")?;
    let sets = required(candidate.sets, "sets")?;
    let duration_sec = required(candidate.duration_sec, "duration_sec")?;

    if sets != 1 {
        bail!("LLM recommended more than one set");
    }
    if reps > 20 {
        bail!("LLM recommended too many reps");
    }
    if duration_sec + 15 > event.expected_duration_sec {
        bail!("LLM recommendation does not fit downtime");
    }
    let movement = movements
        .iter()
        .find(|movement| movement.id == exercise_id)
        .ok_or_else(|| anyhow!("LLM recommendation is not in the movement catalog"))?;
    if movement.sidedness == MovementSidedness::Unilateral && candidate.side.is_none() {
        bail!("unilateral movement is missing a side");
    }
    if movement.sidedness == MovementSidedness::Bilateral
        && candidate
            .side
            .is_some_and(|side| side != RecommendationSide::Bilateral)
    {
        bail!("bilateral movement cannot have a single-side assignment");
    }
    let safety_text = format!(
        "{} {}",
        candidate.safety_notes.unwrap_or_default(),
        candidate.rationale.unwrap_or_default()
    )
    .to_lowercase();
    if safety_text.contains("failure") {
        bail!("LLM recommendation mentions failure training");
    }

    Ok(Some(Recommendation {
        id: None,
        movement_id: movement.id.clone(),
        movement_name: movement.name.clone(),
        primary_muscle: movement.primary_muscle.clone(),
        muscles: movement.muscles.clone(),
        reps,
        weight_kg: parse_weight_kg(candidate.weight_text.as_deref()),
        estimated_seconds: duration_sec,
        agent: event.agent,
        project: event.project.clone(),
        side: candidate.side,
        created_at: Utc::now(),
    }))
}

fn required<T>(value: Option<T>, name: &str) -> Result<T> {
    value.ok_or_else(|| anyhow!("LLM recommendation missing {name}"))
}

fn conflicts_with_limitations(config: &Config, primary_muscle: &str, muscles: &[String]) -> bool {
    let targets = std::iter::once(primary_muscle)
        .chain(muscles.iter().map(String::as_str))
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    config
        .profile
        .cautious_body_parts
        .iter()
        .chain(config.profile.injuries.iter())
        .map(|item| item.to_lowercase())
        .any(|item| {
            targets
                .iter()
                .any(|target| item.contains(target) || target.contains(&item))
        })
}

fn parse_weight_kg(text: Option<&str>) -> Option<f32> {
    let text = text?.to_lowercase();
    let mut previous_number = None;
    for token in text
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
    {
        if let Ok(value) = token.parse::<f32>() {
            previous_number = Some(value);
            continue;
        }
        if matches!(token, "kg" | "kgs" | "kilogram" | "kilograms") {
            return previous_number;
        }
    }
    previous_number
}

fn find_first_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(|value| value.as_str()) {
                return Some(text.to_string());
            }
            for value in map.values() {
                if let Some(found) = find_first_text(value) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(values) => values.iter().find_map(find_first_text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Recommender, RecommenderBackend};
    use crate::models::{Agent, RecommenderTokenUsageSummary, SetStatus};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn test_store() -> Store {
        let dir = tempdir().unwrap().keep();
        let path = dir.join("test.sqlite3");
        let store = Store::open(&path).unwrap();
        store.seed_movements().unwrap();
        store
    }

    fn test_paths() -> Paths {
        Paths::from_root(tempdir().unwrap().keep().join("svarog"))
    }

    #[test]
    fn adaptive_reps_respects_reductions_and_requires_repeated_positive_evidence() {
        let store = test_store();
        let movement = store.movements().unwrap().into_iter().next().unwrap();
        let mut recommendation = Recommendation {
            id: None,
            movement_id: movement.id.clone(),
            movement_name: movement.name,
            primary_muscle: movement.primary_muscle,
            muscles: movement.muscles,
            reps: 10,
            weight_kg: None,
            estimated_seconds: movement.estimated_seconds,
            agent: Agent::Custom,
            project: None,
            side: None,
            created_at: Utc::now(),
        };
        recommendation.id = Some(store.insert_recommendation(&recommendation).unwrap());
        store
            .record_set_with_reps(&recommendation, SetStatus::Done, 6)
            .unwrap();
        assert_eq!(adaptive_reps(&store, &movement.id, 8).unwrap(), 6);

        for actual in [8, 9] {
            recommendation.id = None;
            recommendation.reps = actual - 1;
            recommendation.id = Some(store.insert_recommendation(&recommendation).unwrap());
            store
                .record_set_with_reps(&recommendation, SetStatus::Done, actual)
                .unwrap();
        }
        assert_eq!(adaptive_reps(&store, &movement.id, 8).unwrap(), 9);
    }

    #[test]
    fn custom_archetype_uses_athlete_for_local_scoring() {
        let entry = ExerciseCatalogEntry {
            id: "test".into(),
            force: Some("push".into()),
            mechanic: Some("compound".into()),
            equipment: Some("body only".into()),
            primary_muscles: vec!["quadriceps".into()],
            secondary_muscles: vec![],
            category: "strength".into(),
            instructions: vec![],
            images: vec![],
        };
        let athlete = Config::default();
        let mut custom = Config::default();
        custom.forge.archetype = crate::archetypes::ArchetypeId::Custom;
        custom.forge.custom_archetype = Some("Goku".into());
        assert_eq!(
            archetype_score(&athlete, &entry),
            archetype_score(&custom, &entry)
        );
    }

    #[test]
    fn local_scoring_reflects_representative_archetype_biases() {
        let entry = |category: &str| ExerciseCatalogEntry {
            id: category.into(),
            force: None,
            mechanic: None,
            equipment: Some("body only".into()),
            primary_muscles: vec![],
            secondary_muscles: vec![],
            category: category.into(),
            instructions: vec![],
            images: vec![],
        };
        let mut runner = Config::default();
        runner.forge.archetype = crate::archetypes::ArchetypeId::Runner;
        assert!(
            archetype_score(&runner, &entry("cardio"))
                > archetype_score(&runner, &entry("strength"))
        );

        let mut yogi = Config::default();
        yogi.forge.archetype = crate::archetypes::ArchetypeId::Yogi;
        assert!(
            archetype_score(&yogi, &entry("stretching"))
                > archetype_score(&yogi, &entry("strength"))
        );
    }

    fn event() -> AgentEvent {
        AgentEvent {
            agent: Agent::Codex,
            event: "task_start".into(),
            expected_duration_sec: 120,
            project: Some("svarog".into()),
            created_at: Utc::now(),
        }
    }

    fn codex_config_returning(message: &str) -> Config {
        let root = tempdir().unwrap().keep();
        let command = root.join("fake-codex-response.sh");
        let item = json!({
            "type": "item.completed",
            "item": { "type": "agent_message", "text": message }
        });
        let completed = json!({
            "type": "turn.completed",
            "usage": {
                "input_tokens": 100,
                "cached_input_tokens": 80,
                "output_tokens": 10,
                "reasoning_output_tokens": 0
            }
        });
        fs::write(
            &command,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}'\n",
                item, completed
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;
        config
    }

    fn valid_wrist_candidate() -> serde_json::Value {
        json!({
            "action": "recommend",
            "exercise_id": "Dead_Bug",
            "reps": 5,
            "sets": 1,
            "weight_text": null,
            "duration_sec": 35,
            "safety_notes": "Move gently.",
            "rationale": "Fits the wait.",
            "side": "bilateral"
        })
    }

    #[test]
    fn exercise_profile_prompt_excludes_internal_configuration() {
        let mut config = Config::default();
        config.profile.injuries = vec!["left knee".into()];
        let prompt = PromptRenderer::new(&test_paths().config_dir)
            .exercise_profile(&exercise_profile_context(&config))
            .unwrap();

        assert!(prompt.contains("left knee"));
        assert!(prompt.contains("default_expected_duration_sec"));
        assert!(!prompt.contains("\"agents\""));
        assert!(!prompt.contains("\"recommender\""));
        assert!(!prompt.contains("\"onboarding\""));
    }

    #[test]
    fn openai_queue_payload_uses_responses_reasoning_and_schema() {
        let store = test_store();
        let config = Config::default();
        let event = event();
        let context = build_context(&store, &config, &event).unwrap();
        let prompt = PromptRenderer::new(&test_paths().config_dir)
            .recommendation_queue(&context, QUEUE_TARGET)
            .unwrap();
        let body = openai_queue_request_body(&config, &prompt, QUEUE_TARGET).unwrap();

        assert_eq!(body["model"], "gpt-5.6-luna");
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(
            body["text"]["format"]["name"],
            "svarog_recommendation_queue"
        );
        assert_eq!(body["input"][0]["content"], prompt);
        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["recommendations"]["minItems"],
            QUEUE_TARGET
        );
        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["recommendations"]["maxItems"],
            QUEUE_TARGET
        );
    }

    #[test]
    fn recommendation_schema_requires_the_requested_queue_size() {
        for needed in [2, QUEUE_TARGET] {
            let schema = recommendation_queue_schema(needed);
            assert_eq!(schema["properties"]["recommendations"]["minItems"], needed);
            assert_eq!(schema["properties"]["recommendations"]["maxItems"], needed);
        }
    }

    #[test]
    fn parses_openai_responses_usage_details() {
        let response = json!({
            "usage": {
                "input_tokens": 1234,
                "input_tokens_details": { "cached_tokens": 200 },
                "output_tokens": 56,
                "output_tokens_details": { "reasoning_tokens": 12 }
            }
        });

        assert_eq!(
            parse_openai_usage(&response).unwrap(),
            Some(RecommenderTokenUsage {
                input_tokens: 1234,
                cached_input_tokens: 200,
                output_tokens: 56,
                reasoning_output_tokens: 12,
            })
        );
        assert_eq!(parse_openai_usage(&json!({})).unwrap(), None);
        assert_eq!(parse_openai_usage(&json!({ "usage": null })).unwrap(), None);
    }

    #[test]
    fn openai_api_errors_include_the_complete_sanitized_message() {
        let detail = format!("unsupported\n schema {}", "x".repeat(600));
        let message = openai_api_error_message(
            reqwest::StatusCode::BAD_REQUEST,
            &json!({
                "error": {
                    "message": detail,
                    "code": "credit_balance_exhausted"
                }
            })
            .to_string(),
        );

        assert!(message.starts_with(
            "OpenAI Responses API returned 400 Bad Request (credit_balance_exhausted): unsupported schema"
        ));
        assert!(!message.contains('\n'));
        assert!(message.ends_with(&"x".repeat(600)));
        assert_eq!(
            openai_api_error_message(reqwest::StatusCode::BAD_GATEWAY, "not json"),
            "OpenAI Responses API returned 502 Bad Gateway"
        );
    }

    #[test]
    fn recommendation_prompt_uses_compact_bounded_history() {
        let store = test_store();
        let movement = store.movements().unwrap().remove(0);
        let recommendation = Recommendation {
            id: None,
            movement_id: movement.id,
            movement_name: movement.name,
            primary_muscle: movement.primary_muscle,
            muscles: movement.muscles,
            reps: movement.base_reps,
            weight_kg: None,
            estimated_seconds: movement.estimated_seconds,
            agent: Agent::Codex,
            project: Some("svarog".into()),
            side: None,
            created_at: Utc::now(),
        };
        for reps in 1..=8 {
            store
                .record_set_with_reps(&recommendation, SetStatus::Done, reps)
                .unwrap();
        }
        let context = build_context(&store, &Config::default(), &event()).unwrap();
        let prompt = PromptRenderer::new(&test_paths().config_dir)
            .recommendation_queue(&context, QUEUE_TARGET)
            .unwrap();
        let context_json = prompt.split_once("Context JSON:\n").unwrap().1;
        let rendered: serde_json::Value = serde_json::from_str(context_json).unwrap();

        assert_eq!(rendered["recent_sets"].as_array().unwrap().len(), 8);
        assert!(rendered["profile"].get("height_cm").is_some());
        assert!(rendered["profile"].get("weight_kg").is_some());
        assert!(rendered["profile"].get("age").is_some());
        assert!(rendered["recent_sets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|set| set.get("agent").is_none() && set.get("project").is_none()));
        assert!(rendered.get("today_events").is_none());
        assert!(rendered.get("today_sets").is_none());
        assert!(prompt.len() < 60_000, "prompt was {} bytes", prompt.len());
        assert!(prompt.contains("Return exactly 10 valid, distinct recommendations"));
        assert!(!prompt.contains("return fewer"));
    }

    #[test]
    fn equipment_text_normalizes_categories_and_weights() {
        let capabilities = normalize_equipment(
            "12 kg kettlebell, 2x8 kg dumbbells, resistance band, and a medical ball",
        );
        assert!(capabilities.iter().any(|capability| {
            capability.kind == "kettlebell"
                && capability.count == 1
                && capability.weights_kg.contains(&12.0)
        }));
        assert!(capabilities.iter().any(|capability| {
            capability.kind == "dumbbell"
                && capability.count == 2
                && capability.weights_kg.contains(&8.0)
        }));
        assert!(capabilities
            .iter()
            .any(|capability| capability.kind == "band"));
        assert!(capabilities
            .iter()
            .any(|capability| capability.kind == "medicine_ball"));
    }

    #[test]
    fn resolved_dumbbell_profile_is_filtered_locally_without_catalog_input() {
        let (movements, equipment) = validate_exercise_profile(LlmExerciseProfile {
            equipment: vec![LlmEquipmentCapability {
                kind: "dumbbell".into(),
                weights_kg: vec![10.0],
                count: 1,
            }],
        })
        .unwrap();

        assert!(equipment.iter().any(|item| item.kind == "bodyweight"));
        assert!(equipment
            .iter()
            .any(|item| { item.kind == "dumbbell" && item.weights_kg == vec![10.0] }));
        assert!(movements
            .iter()
            .any(|movement| movement.equipment == ["bodyweight"]));
        assert!(movements
            .iter()
            .any(|movement| movement.equipment == ["dumbbell"]));
        assert!(movements.iter().all(|movement| {
            movement.equipment == ["bodyweight"] || movement.equipment == ["dumbbell"]
        }));
    }

    #[test]
    fn resolved_kettlebell_profile_respects_equipment_quantity() {
        let resolve = |count| {
            validate_exercise_profile(LlmExerciseProfile {
                equipment: vec![LlmEquipmentCapability {
                    kind: "kettlebell".into(),
                    weights_kg: vec![12.0],
                    count,
                }],
            })
            .unwrap()
            .0
        };

        assert!(!resolve(1)
            .iter()
            .any(|movement| movement.id == "Double_Kettlebell_Jerk"));
        assert!(resolve(2)
            .iter()
            .any(|movement| movement.id == "Double_Kettlebell_Jerk"));
    }

    #[test]
    fn legacy_saved_equipment_capabilities_default_to_one_item() {
        let capability: EquipmentCapability =
            serde_json::from_str(r#"{"kind":"kettlebell","weights_kg":[12.0]}"#).unwrap();
        assert_eq!(capability.count, 1);
    }

    #[test]
    fn queue_context_uses_only_canonical_catalog_fields() {
        let store = test_store();
        let context = build_context(&store, &Config::default(), &event()).unwrap();
        let value = serde_json::to_value(context).unwrap();
        let movement = value["movements"][0].as_object().unwrap();
        assert_eq!(
            movement
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from([
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

    #[test]
    fn validator_rejects_unavailable_catalog_equipment() {
        let mut config = Config::default();
        config.profile.equipment_text = "bodyweight only".into();
        let candidate: LlmRecommendation = serde_json::from_value(json!({
            "action": "recommend",
            "exercise_id": "Dumbbell_Bicep_Curl",
            "reps": 8,
            "sets": 1,
            "weight_text": null,
            "duration_sec": 35,
            "safety_notes": "Move gently.",
            "rationale": "Fits the wait.",
            "side": "left"
        }))
        .unwrap();
        let error = validate_candidate(
            &config,
            &event(),
            candidate,
            &test_store().movements().unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not in the movement catalog"));
    }

    #[test]
    fn parses_json_from_noisy_codex_output() {
        let parsed: LlmRecommendation = parse_llm_json(
            r#"thinking...
            {"action":"no_recommendation","exercise_id":null,"reps":null,"sets":null,"weight_text":null,"duration_sec":null,"safety_notes":null,"rationale":null,"side":null}"#,
        )
        .unwrap();
        assert_eq!(parsed.action, LlmAction::NoRecommendation);
    }

    #[test]
    fn rejects_a_bare_recommendation_array() {
        let result = parse_llm_json::<LlmRecommendationBatch>(
            r#"[{"movement_id":"scapular_squeeze","reps":8,"estimated_seconds":40}]"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn fallback_notices_hide_internal_parser_details() {
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        let invalid = anyhow!("parsing codex JSON: invalid recommendation JSON");
        let timeout = anyhow!("codex recommender timed out after 60 seconds");

        assert_eq!(
            fallback_notice(&config, &invalid).as_deref(),
            Some("Codex returned an invalid response.")
        );
        assert_eq!(
            fallback_notice(&config, &timeout).as_deref(),
            Some("Codex timed out.")
        );
    }

    #[test]
    fn svarog_replaces_configured_codex_output_schema() {
        let args = vec![
            "exec".into(),
            "--output-schema".into(),
            "other.json".into(),
            "--sandbox".into(),
            "read-only".into(),
            "--output-schema=also-other.json".into(),
        ];

        assert_eq!(
            codex_args_without_output_schema(&args),
            vec!["exec", "--sandbox", "read-only"]
        );
    }

    #[test]
    fn codex_recommender_marks_its_internal_process() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-codex.sh");
        fs::write(
            &command,
            r#"#!/bin/sh
if [ "$SVAROG_RECOMMENDER" != "1" ]; then
  exit 42
fi
if [ "$1" != "--json" ]; then
  exit 43
fi
schema=
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-schema" ]; then
    shift
    schema="$1"
  fi
  shift
done
if [ ! -s "$schema" ]; then
  exit 44
fi
if ! grep -q '"recommendations"' "$schema"; then
  exit 45
fi
i=0
while [ "$i" -lt 2000 ]; do
  printf '%s\n' '{"type":"thread.started","thread_id":"thread-1"}'
  i=$((i + 1))
done
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"recommendations\":[]}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122,"reasoning_output_tokens":0}}'
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();

        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;

        let store = test_store();
        let batch: LlmRecommendationBatch = call_codex_json(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        )
        .unwrap();
        assert!(batch.recommendations.is_empty());
        let usage = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap();
        assert_eq!(usage.today.input_tokens, 24_763);
        assert_eq!(usage.today.output_tokens, 122);
    }

    #[test]
    fn dedicated_codex_call_cancels_and_reaps_its_process_group() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-cancellable-codex.sh");
        fs::write(
            &command,
            r#"#!/bin/sh
sleep 30
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 10_000;
        let store = test_store();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.store(true, Ordering::Release);
        });
        let started = Instant::now();

        let error = call_codex_json_for_model::<LlmRecommendationBatch>(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
            "gpt-5.6-luna",
            cancel.as_ref(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            store
                .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
                .unwrap(),
            RecommenderTokenUsageSummary::default()
        );
    }

    #[test]
    fn codex_recommender_skips_the_git_repository_check() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-non-git-codex.sh");
        fs::write(
            &command,
            r#"#!/bin/sh
found=false
for argument in "$@"; do
  if [ "$argument" = "--skip-git-repo-check" ]; then
    found=true
  fi
done
if [ "$found" != "true" ]; then
  printf '%s\n' 'git repository check was not disabled' >&2
  exit 1
fi
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"recommendations\":[]}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0}}'
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.timeout_ms = 5_000;
        let store = test_store();

        let result: LlmRecommendationBatch = call_codex_json(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        )
        .unwrap();

        assert!(result.recommendations.is_empty());
    }

    #[test]
    fn codex_recommender_uses_luna_then_retries_the_inherited_model() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-model-retry.sh");
        let log = root.path().join("models.log");
        let script = r#"#!/bin/sh
model=inherit
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--model" ]; then
    shift
    model="$1"
  fi
  shift
done
printf '%s\n' "$model" >> "__LOG__"
if [ "$model" = "gpt-5.6-luna" ]; then
  printf '%s\n' 'model unavailable' >&2
  exit 1
fi
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"recommendations\":[]}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}'
"#
        .replace("__LOG__", &log.display().to_string());
        fs::write(&command, script).unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;
        let store = test_store();

        let batch: LlmRecommendationBatch = call_codex_json(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        )
        .unwrap();

        assert!(batch.recommendations.is_empty());
        assert_eq!(fs::read_to_string(log).unwrap(), "gpt-5.6-luna\ninherit\n");
        let usage = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap();
        assert_eq!(usage.today.input_tokens, 100);
        assert_eq!(usage.today.output_tokens, 5);
    }

    #[test]
    fn codex_model_retry_shares_the_original_timeout_budget() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-slow-retry.sh");
        let script = r#"#!/bin/sh
model=inherit
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--model" ]; then
    shift
    model="$1"
  fi
  shift
done
if [ "$model" = "gpt-5.6-luna" ]; then
  sleep 0.8
  exit 1
fi
exec sleep 2
"#;
        fs::write(&command, script).unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 1_500;
        let store = test_store();
        let started = Instant::now();

        let result = call_codex_json::<LlmRecommendationBatch>(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        );

        let error = result.unwrap_err();
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("timed out")),
            "unexpected error: {error:#}"
        );
        assert!(started.elapsed() < Duration::from_millis(1_900));
    }

    #[test]
    fn codex_timeout_does_not_retry_with_the_inherited_model() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-timeout.sh");
        let log = root.path().join("models.log");
        let script = r#"#!/bin/sh
model=inherit
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--model" ]; then
    shift
    model="$1"
  fi
  shift
done
printf '%s\n' "$model" >> "__LOG__"
exec sleep 5
"#
        .replace("__LOG__", &log.display().to_string());
        fs::write(&command, script).unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 2_000;
        let store = test_store();

        let result = call_codex_json::<LlmRecommendationBatch>(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        );

        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_eq!(fs::read_to_string(log).unwrap(), "gpt-5.6-luna\n");
    }

    #[test]
    fn parses_codex_jsonl_message_and_usage() {
        let output = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"recommendations\\\":[]}\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":24763,\"cached_input_tokens\":24448,\"output_tokens\":122,\"reasoning_output_tokens\":7}}\n",
        );

        let (message, usage) = parse_codex_jsonl(output).unwrap();

        assert_eq!(message, r#"{"recommendations":[]}"#);
        assert_eq!(
            usage.unwrap(),
            RecommenderTokenUsage {
                input_tokens: 24_763,
                cached_input_tokens: 24_448,
                output_tokens: 122,
                reasoning_output_tokens: 7,
            }
        );
    }

    #[test]
    fn completed_usage_is_recorded_before_recommendation_parsing() {
        let root = tempdir().unwrap();
        let command = root.path().join("fake-invalid-codex.sh");
        fs::write(
            &command,
            r#"#!/bin/sh
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"not recommendation JSON"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":900,"output_tokens":20,"reasoning_output_tokens":5}}'
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let mut config = Config::default();
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;
        let store = test_store();

        let result = call_codex_json::<LlmRecommendationBatch>(
            &store,
            &config,
            "ignored",
            &recommendation_queue_schema(QUEUE_TARGET),
        );

        assert!(result.is_err());
        let usage = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap();
        assert_eq!(usage.today.input_tokens, 1_000);
        assert_eq!(usage.today.output_tokens, 20);
    }

    #[test]
    fn validator_keeps_injuries_advisory_for_the_llm() {
        let mut config = Config::default();
        config.profile.injuries = vec!["legs and spine".to_string()];
        let candidate = LlmRecommendation {
            action: LlmAction::Recommend,
            exercise_id: Some("Bodyweight_Squat".into()),
            reps: Some(5),
            sets: Some(1),
            weight_text: None,
            duration_sec: Some(30),
            safety_notes: Some("stop if pain appears".into()),
            rationale: Some("short movement".into()),
            side: None,
        };
        let recommendation = validate_candidate(
            &config,
            &event(),
            candidate,
            &test_store().movements().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(recommendation.movement_id, "Bodyweight_Squat");
    }

    #[test]
    fn local_backend_fills_recommendation_queue() {
        let store = test_store();
        let config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };

        fill_recommendation_queue(&store, &config, &test_paths()).unwrap();

        let queued = store.queued_recommendation_count().unwrap();
        assert!(queued > 0 && queued <= QUEUE_TARGET);
        assert!(store.latest_open_recommendation().unwrap().is_none());
    }

    #[test]
    fn low_water_mark_preserves_existing_item_and_appends_a_new_batch() {
        let store = test_store();
        let config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };

        fill_recommendation_queue(&store, &config, &test_paths()).unwrap();
        let first = store.queued_recommendations().unwrap().remove(0);
        store.clear_queued_recommendations().unwrap();
        store.insert_queued_recommendation(&first).unwrap();

        fill_recommendation_queue(&store, &config, &test_paths()).unwrap();
        let queued = store.queued_recommendations().unwrap();
        assert!(queued.len() > 1);
        assert!(queued.len() <= (QUEUE_TARGET + 1) as usize);

        let count = queued.len();
        fill_recommendation_queue(&store, &config, &test_paths()).unwrap();
        assert_eq!(store.queued_recommendation_count().unwrap() as usize, count);
    }

    #[test]
    fn generating_a_local_queue_does_not_persist_candidates() {
        let store = test_store();
        let config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert!(!generated.recommendations.is_empty());
        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::Local);
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
    }

    #[test]
    fn partial_codex_queue_is_filled_locally_without_duplicates() {
        let store = test_store();
        let message = json!({ "recommendations": [valid_wrist_candidate()] }).to_string();
        let config = codex_config_returning(&message);

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::Hybrid);
        assert_eq!(generated.llm_count, 1);
        assert_eq!(generated.local_count, generated.recommendations.len() - 1);
        assert!(generated
            .notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("Codex suggested 1; Svarog filled ")));
        let movement_ids = generated
            .recommendations
            .iter()
            .map(|recommendation| &recommendation.movement_id)
            .collect::<std::collections::HashSet<_>>();
        let muscles = generated
            .recommendations
            .iter()
            .map(|recommendation| &recommendation.primary_muscle)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(movement_ids.len(), generated.recommendations.len());
        assert!(muscles.len() <= generated.recommendations.len());
    }

    #[test]
    fn empty_codex_queue_uses_local_fallback() {
        let store = test_store();
        let config = codex_config_returning(r#"{"recommendations":[]}"#);

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert!(!generated.recommendations.is_empty());
        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::LocalFallback);
        assert_eq!(generated.llm_count, 0);
        assert_eq!(generated.local_count, generated.recommendations.len());
        assert_eq!(
            generated.notice.as_deref(),
            Some("Codex did not suggest any safe forges.")
        );
    }

    #[test]
    fn future_local_queue_keeps_cooling_muscles_after_recovered_options() {
        let store = test_store();
        let movement = store
            .movements()
            .unwrap()
            .into_iter()
            .find(|movement| movement.mobility)
            .unwrap();
        let cooling_muscle = movement.primary_muscle.clone();
        let completed = Recommendation {
            id: None,
            movement_id: movement.id,
            movement_name: movement.name,
            primary_muscle: movement.primary_muscle,
            muscles: movement.muscles,
            reps: movement.base_reps,
            weight_kg: None,
            estimated_seconds: movement.estimated_seconds,
            agent: Agent::Codex,
            project: None,
            side: None,
            created_at: Utc::now(),
        };
        store.insert_queued_recommendation(&completed).unwrap();
        let completed = store
            .promote_next_queued_recommendation(Agent::Codex, None)
            .unwrap()
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        let queued = local_queue(&store, &Config::default(), &event(), 20, &[]).unwrap();

        assert_ne!(queued.first().unwrap().primary_muscle, cooling_muscle);
        assert!(queued
            .iter()
            .any(|recommendation| recommendation.primary_muscle == cooling_muscle));
    }

    #[test]
    fn invalid_codex_candidates_are_skipped_before_local_top_up() {
        let store = test_store();
        let mut invalid = valid_wrist_candidate();
        invalid["exercise_id"] = json!("not-a-canonical-id");
        invalid["reps"] = json!(99);
        let message = json!({
            "recommendations": [valid_wrist_candidate(), invalid]
        })
        .to_string();
        let config = codex_config_returning(&message);

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.llm_count, 1);
        assert_eq!(generated.local_count, generated.recommendations.len() - 1);
        assert!(!generated
            .recommendations
            .iter()
            .any(|recommendation| recommendation.movement_id == "not-a-canonical-id"));
    }

    #[test]
    fn queue_generation_returns_local_fallback_and_notice_without_persisting() {
        let root = tempdir().unwrap();
        let store = test_store();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        config.recommender.codex.command = root.path().join("missing-codex").display().to_string();
        config.recommender.local_fallback = true;
        config.recommender.show_llm_failures = true;

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::LocalFallback);
        assert_eq!(generated.notice.as_deref(), Some("Codex was unavailable."));
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
    }

    #[test]
    fn invalid_prompt_override_uses_local_queue_fallback() {
        let store = test_store();
        let paths = test_paths();
        fs::create_dir_all(paths.config_dir.join("prompts")).unwrap();
        fs::write(
            paths.config_dir.join("prompts/recommendation_queue.j2"),
            "{{ unknown_prompt_value }}",
        )
        .unwrap();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        config.recommender.local_fallback = true;
        config.recommender.show_llm_failures = true;

        let generated =
            generate_recommendation_queue(&store, &config, &paths, QUEUE_TARGET).unwrap();

        assert!(generated.recommendations.len() <= QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::LocalFallback);
        assert_eq!(generated.notice.as_deref(), Some("Codex was unavailable."));
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
    }

    #[test]
    fn local_initial_profile_keeps_injuries_advisory() {
        let mut config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };
        config.profile.injuries = vec!["legs".to_string()];

        let store = test_store();
        let (movements, notice) = initial_exercise_profile(&store, &config, &test_paths());
        let squat = movements
            .iter()
            .find(|movement| movement.id == "Bodyweight_Squat")
            .unwrap();

        assert_eq!(squat.status, MovementStatus::Allowed);
        assert!(notice.is_none());
    }
}
