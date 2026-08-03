use crate::config::{Config, Paths, Profile, RecommenderBackend};
use crate::models::{
    AgentEvent, Movement, MovementStatus, Recommendation, RecommenderTokenProvider,
    RecommenderTokenUsage,
};
use crate::prompt_templates::PromptRenderer;
use crate::storage::{SetSummary, Store, MUSCLE_COOLDOWN_MINUTES};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const QUEUE_TARGET: u32 = 5;

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
    movements: Vec<Movement>,
}

#[derive(Debug, Serialize)]
struct ExerciseProfileContext<'a> {
    profile: &'a Profile,
    preferences: ExerciseProfilePreferences,
}

#[derive(Debug, Serialize)]
struct ExerciseProfilePreferences {
    forge_intensity: u32,
    default_expected_duration_sec: u32,
    max_daily_sets: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ContextProfile {
    goals: Vec<String>,
    equipment_text: String,
    exercise_preferences: String,
    work_setup: String,
    one_hand_available: bool,
    two_hand_available: bool,
    cautious_body_parts: Vec<String>,
    injuries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ContextPreferences {
    forge_intensity: u32,
    max_daily_sets: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ContextSet {
    movement_id: String,
    muscles: Vec<String>,
    status: String,
    reps: u32,
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
    movement_name: Option<String>,
    reps: Option<u32>,
    sets: Option<u32>,
    weight_text: Option<String>,
    duration_sec: Option<u32>,
    primary_muscle: Option<String>,
    muscles: Option<Vec<String>>,
    equipment_used: Option<String>,
    safety_notes: Option<String>,
    rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRecommendationBatch {
    recommendations: Vec<LlmRecommendation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmExerciseProfile {
    movements: Vec<LlmExerciseMovement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmExerciseMovement {
    name: String,
    primary_muscle: String,
    muscles: Vec<String>,
    equipment: Vec<String>,
    base_reps: u32,
    estimated_seconds: u32,
    status: MovementStatus,
    mobility: bool,
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
    let queued = store.queued_recommendation_count()?;
    if queued >= QUEUE_TARGET {
        return Ok(None);
    }
    let generated = generate_recommendation_queue(store, config, paths, QUEUE_TARGET - queued)?;
    for rec in generated.recommendations {
        store.insert_queued_recommendation(&rec)?;
    }
    Ok(generated.notice)
}

pub fn generate_recommendation_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
    needed: u32,
) -> Result<QueueGeneration> {
    let event = AgentEvent {
        agent: crate::models::Agent::Custom,
        event: "prefetch".to_string(),
        expected_duration_sec: config.preferences.default_expected_duration_sec,
        project: None,
        created_at: Utc::now(),
    };
    let (mut recommendations, notice, source, llm_count, local_count) =
        match config.recommender.backend {
            RecommenderBackend::Off => (Vec::new(), None, QueueGenerationSource::Local, 0, 0),
            RecommenderBackend::Local => {
                let local = local_queue(store, config, &event, needed, &[])?;
                let local_count = local.len();
                (local, None, QueueGenerationSource::Local, 0, local_count)
            }
            RecommenderBackend::Codex | RecommenderBackend::Openai => {
                match llm_queue(store, config, paths, &event, needed) {
                    Ok(mut llm) => {
                        llm.truncate(needed as usize);
                        let llm_count = llm.len();
                        let local =
                            if config.recommender.local_fallback && llm_count < needed as usize {
                                local_queue(store, config, &event, needed - llm_count as u32, &llm)?
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
                                RecommenderBackend::Openai => QueueGenerationSource::OpenAi,
                                _ => unreachable!(),
                            }
                        };
                        let notice = hybrid_notice(config, llm_count, local_count);
                        (llm, notice, source, llm_count, local_count)
                    }
                    Err(err) if config.recommender.local_fallback => {
                        let fallback = local_queue(store, config, &event, needed, &[])?;
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
        RecommenderBackend::Openai => "OpenAI",
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
        RecommenderBackend::Openai => "OpenAI",
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
        (
            !*recovered,
            movement.status == MovementStatus::Caution,
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
        recommendations.push(Recommendation {
            id: None,
            movement_id: movement.id,
            movement_name: movement.name,
            primary_muscle: movement.primary_muscle,
            muscles: movement.muscles,
            reps: movement.base_reps + config.preferences.forge_intensity.clamp(1, 5) - 1,
            weight_kg: None,
            estimated_seconds: movement.estimated_seconds,
            agent: event.agent,
            project: event.project.clone(),
            created_at: Utc::now(),
        });
        if recommendations.len() >= needed as usize {
            break;
        }
    }
    Ok(recommendations)
}

fn llm_queue(
    store: &Store,
    config: &Config,
    paths: &Paths,
    event: &AgentEvent,
    needed: u32,
) -> Result<Vec<Recommendation>> {
    let context = build_context(store, config, event)?;
    let prompt = PromptRenderer::new(&paths.config_dir).recommendation_queue(&context, needed)?;
    let batch = match config.recommender.backend {
        RecommenderBackend::Codex => call_codex_queue(store, config, &prompt, needed),
        RecommenderBackend::Openai => call_openai_queue(store, config, &prompt, needed),
        RecommenderBackend::Local | RecommenderBackend::Off => unreachable!(),
    }?;
    let mut recommendations = Vec::new();
    for candidate in batch.recommendations {
        let Ok(Some(rec)) = validate_candidate(config, event, candidate) else {
            continue;
        };
        if !recommendations.iter().any(|existing: &Recommendation| {
            existing.primary_muscle == rec.primary_muscle || existing.movement_id == rec.movement_id
        }) {
            recommendations.push(rec);
        }
        if recommendations.len() >= needed as usize {
            break;
        }
    }
    Ok(recommendations)
}

pub fn initial_exercise_profile(
    store: &Store,
    config: &Config,
    paths: &Paths,
) -> (Vec<Movement>, Option<String>) {
    if matches!(
        config.recommender.backend,
        RecommenderBackend::Off | RecommenderBackend::Local
    ) {
        return (local_initial_movements(config), None);
    }

    let result = PromptRenderer::new(&paths.config_dir)
        .exercise_profile(&exercise_profile_context(config))
        .and_then(|prompt| match config.recommender.backend {
            RecommenderBackend::Codex => call_codex_exercise_profile(store, config, &prompt),
            RecommenderBackend::Openai => call_openai_exercise_profile(store, config, &prompt),
            RecommenderBackend::Local | RecommenderBackend::Off => unreachable!(),
        })
        .and_then(|profile| validate_exercise_profile(config, profile));

    match result {
        Ok(movements) if !movements.is_empty() => (movements, None),
        Ok(_) => (local_initial_movements(config), None),
        Err(err) => (
            local_initial_movements(config),
            config
                .recommender
                .show_llm_failures
                .then(|| format!("Using conservative automatic exercise selection: {err}")),
        ),
    }
}

fn exercise_profile_context(config: &Config) -> ExerciseProfileContext<'_> {
    ExerciseProfileContext {
        profile: &config.profile,
        preferences: ExerciseProfilePreferences {
            forge_intensity: config.preferences.forge_intensity,
            default_expected_duration_sec: config.preferences.default_expected_duration_sec,
            max_daily_sets: config.preferences.max_daily_sets,
        },
    }
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
    Ok(RecommendationContext {
        profile: ContextProfile {
            goals: profile.goals.clone(),
            equipment_text: profile.equipment_text.clone(),
            exercise_preferences: profile.exercise_preferences.clone(),
            work_setup: profile.work_setup.clone(),
            one_hand_available: profile.one_hand_available,
            two_hand_available: profile.two_hand_available,
            cautious_body_parts: profile.cautious_body_parts.clone(),
            injuries: profile.injuries.clone(),
        },
        preferences: ContextPreferences {
            forge_intensity: preferences.forge_intensity,
            max_daily_sets: preferences.max_daily_sets,
        },
        expected_duration_sec: event.expected_duration_sec,
        today_stats: TodayStats { sets, reps, breaks },
        recent_sets: store
            .today_sets(5)?
            .into_iter()
            .map(ContextSet::from)
            .collect(),
        app_state: ContextAppState {
            kind: state.kind.as_str().to_string(),
            cooldown_muscle: state.cooldown_muscle,
            cooldown_until: state.cooldown_until.map(|dt| dt.to_rfc3339()),
        },
        movements: store.movements()?,
    })
}

impl From<SetSummary> for ContextSet {
    fn from(value: SetSummary) -> Self {
        Self {
            movement_id: value.movement_id,
            muscles: value.muscles,
            status: value.status,
            reps: value.reps,
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

    match call_codex_json_attempt(store, config, prompt, schema_file.path(), model, deadline)? {
        CodexAttempt::Completed(value) => Ok(value),
        CodexAttempt::EarlyFailure(first_error) if model.is_some() => {
            match call_codex_json_attempt(store, config, prompt, schema_file.path(), None, deadline)
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
) -> Result<CodexAttempt<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if Instant::now() >= deadline {
        bail!("codex recommender timed out");
    }
    let mut command = Command::new(&config.recommender.codex.command);
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
            let _ = child.kill();
            let _ = child.wait();
            bail!("codex recommender timed out");
        }
        thread::sleep(Duration::from_millis(50));
    }
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
    prompt: &str,
    needed: u32,
) -> Result<LlmRecommendationBatch> {
    let body = openai_queue_request_body(config, prompt, needed)?;
    call_openai_json(store, config, body).context("parsing OpenAI recommendation queue")
}

fn call_openai_exercise_profile(
    store: &Store,
    config: &Config,
    prompt: &str,
) -> Result<LlmExerciseProfile> {
    let body = openai_exercise_profile_request_body(config, prompt)?;
    call_openai_json(store, config, body).context("parsing OpenAI exercise profile")
}

fn call_openai_json<T>(store: &Store, config: &Config, body: serde_json::Value) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let api_key = std::env::var(&config.recommender.openai.api_key_env)
        .with_context(|| format!("missing {}", config.recommender.openai.api_key_env))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(config.recommender.timeout_ms))
        .build()?;
    let response: serde_json::Value = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .context("calling OpenAI Responses API")?
        .error_for_status()
        .context("OpenAI Responses API returned an error")?
        .json()
        .context("parsing OpenAI response JSON")?;
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
            "action", "movement_name", "reps", "sets", "weight_text", "duration_sec",
            "primary_muscle", "muscles", "equipment_used", "safety_notes", "rationale"
        ],
        "properties": {
            "action": { "type": "string", "enum": ["recommend", "no_recommendation"] },
            "movement_name": { "type": ["string", "null"] },
            "reps": { "type": ["integer", "null"], "minimum": 1, "maximum": 30 },
            "sets": { "type": ["integer", "null"], "minimum": 1, "maximum": 3 },
            "weight_text": { "type": ["string", "null"] },
            "duration_sec": { "type": ["integer", "null"], "minimum": 1, "maximum": 600 },
            "primary_muscle": { "type": ["string", "null"] },
            "muscles": {
                "type": ["array", "null"],
                "items": { "type": "string" }
            },
            "equipment_used": { "type": ["string", "null"] },
            "safety_notes": { "type": ["string", "null"] },
            "rationale": { "type": ["string", "null"] }
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
        "required": ["movements"],
        "properties": {
            "movements": {
                "type": "array",
                "minItems": 4,
                "maxItems": 16,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "name", "primary_muscle", "muscles", "equipment", "base_reps",
                        "estimated_seconds", "status", "mobility"
                    ],
                    "properties": {
                        "name": { "type": "string" },
                        "primary_muscle": { "type": "string" },
                        "muscles": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "maxItems": 6
                        },
                        "equipment": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "maxItems": 4
                        },
                        "base_reps": { "type": "integer", "minimum": 1, "maximum": 20 },
                        "estimated_seconds": { "type": "integer", "minimum": 10, "maximum": 120 },
                        "status": { "type": "string", "enum": ["allowed", "caution", "blocked"] },
                        "mobility": { "type": "boolean" }
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
    config: &Config,
    profile: LlmExerciseProfile,
) -> Result<Vec<Movement>> {
    profile
        .movements
        .into_iter()
        .map(|movement| validate_exercise_movement(config, movement))
        .collect()
}

fn validate_exercise_movement(config: &Config, movement: LlmExerciseMovement) -> Result<Movement> {
    if movement.name.trim().is_empty() {
        bail!("LLM exercise profile included an unnamed movement");
    }
    if movement.base_reps == 0 || movement.base_reps > 20 {
        bail!("LLM exercise profile included invalid reps");
    }
    if !(10..=120).contains(&movement.estimated_seconds) {
        bail!("LLM exercise profile included invalid duration");
    }
    let status = if conflicts_with_limitations(config, &movement.primary_muscle, &movement.muscles)
    {
        MovementStatus::Blocked
    } else {
        movement.status
    };
    Ok(Movement {
        id: slugify(&movement.name),
        name: movement.name.trim().to_string(),
        primary_muscle: movement.primary_muscle.trim().to_lowercase(),
        muscles: movement
            .muscles
            .into_iter()
            .map(|muscle| muscle.trim().to_lowercase())
            .filter(|muscle| !muscle.is_empty())
            .collect(),
        equipment: movement
            .equipment
            .into_iter()
            .map(|equipment| equipment.trim().to_lowercase().replace(' ', "_"))
            .filter(|equipment| !equipment.is_empty())
            .collect(),
        base_reps: movement.base_reps,
        estimated_seconds: movement.estimated_seconds,
        status,
        mobility: movement.mobility,
    })
}

fn local_initial_movements(config: &Config) -> Vec<Movement> {
    let preference_text = format!(
        "{} {} {}",
        config.profile.exercise_preferences,
        config.profile.injuries.join(" "),
        config.profile.cautious_body_parts.join(" ")
    )
    .to_lowercase();
    let upper_body_only = preference_text.contains("upper body");
    let posture = preference_text.contains("posture") || preference_text.contains("stretch");
    let avoid_squats =
        preference_text.contains("avoid squats") || preference_text.contains("no squats");
    let no_jumping = preference_text.contains("no jumping");

    crate::storage::default_movements()
        .into_iter()
        .map(|mut movement| {
            if conflicts_with_limitations(config, &movement.primary_muscle, &movement.muscles)
                || (upper_body_only
                    && movement.muscles.iter().any(|muscle| {
                        muscle.contains("leg")
                            || muscle.contains("quad")
                            || muscle.contains("glute")
                            || muscle.contains("calf")
                    }))
                || (avoid_squats && movement.id.contains("sit_to_stand"))
                || (no_jumping && movement.name.contains("jump"))
            {
                movement.status = MovementStatus::Blocked;
            } else if posture && !movement.mobility && movement.primary_muscle != "upper_back" {
                movement.status = MovementStatus::Caution;
            }
            movement
        })
        .collect()
}

fn validate_candidate(
    config: &Config,
    event: &AgentEvent,
    candidate: LlmRecommendation,
) -> Result<Option<Recommendation>> {
    if candidate.action == LlmAction::NoRecommendation {
        return Ok(None);
    }

    let movement_name = required(candidate.movement_name, "movement_name")?;
    let reps = required(candidate.reps, "reps")?;
    let sets = required(candidate.sets, "sets")?;
    let duration_sec = required(candidate.duration_sec, "duration_sec")?;
    let primary_muscle = required(candidate.primary_muscle, "primary_muscle")?;
    let muscles = required(candidate.muscles, "muscles")?;

    if sets != 1 {
        bail!("LLM recommended more than one set");
    }
    if reps > 20 {
        bail!("LLM recommended too many reps");
    }
    if duration_sec + 15 > event.expected_duration_sec {
        bail!("LLM recommendation does not fit downtime");
    }
    if conflicts_with_limitations(config, &primary_muscle, &muscles) {
        bail!("LLM recommendation conflicts with limitations");
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
        movement_id: slugify(&movement_name),
        movement_name,
        primary_muscle,
        muscles,
        reps,
        weight_kg: parse_weight_kg(candidate.weight_text.as_deref()),
        estimated_seconds: duration_sec,
        agent: event.agent,
        project: event.project.clone(),
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

fn slugify(value: &str) -> String {
    let mut out = value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
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
    use crate::models::{Agent, SetStatus};
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
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;
        config
    }

    fn valid_wrist_candidate() -> serde_json::Value {
        json!({
            "action": "recommend",
            "movement_name": "wrist extensor reset",
            "reps": 5,
            "sets": 1,
            "weight_text": null,
            "duration_sec": 35,
            "primary_muscle": "mobility",
            "muscles": ["forearms", "wrists"],
            "equipment_used": "bodyweight",
            "safety_notes": "Move gently.",
            "rationale": "Fits the wait."
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

        assert_eq!(body["model"], "gpt-5.4-nano");
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
            created_at: Utc::now(),
        };
        for reps in 1..=8 {
            store
                .record_set_with_reps(&recommendation, SetStatus::Skipped, reps)
                .unwrap();
        }
        let context = build_context(&store, &Config::default(), &event()).unwrap();
        let prompt = PromptRenderer::new(&test_paths().config_dir)
            .recommendation_queue(&context, QUEUE_TARGET)
            .unwrap();
        let context_json = prompt.split_once("Context JSON:\n").unwrap().1;
        let rendered: serde_json::Value = serde_json::from_str(context_json).unwrap();

        assert_eq!(rendered["recent_sets"].as_array().unwrap().len(), 5);
        assert!(rendered.get("today_events").is_none());
        assert!(rendered.get("today_sets").is_none());
        assert!(prompt.len() < 6_000, "prompt was {} bytes", prompt.len());
        assert!(prompt.contains("Return exactly 5 distinct recommendations"));
        assert!(!prompt.contains("return fewer"));
    }

    #[test]
    fn parses_json_from_noisy_codex_output() {
        let parsed: LlmRecommendation = parse_llm_json(
            r#"thinking...
            {"action":"no_recommendation","movement_name":null,"reps":null,"sets":null,"weight_text":null,"duration_sec":null,"primary_muscle":null,"muscles":null,"equipment_used":null,"safety_notes":null,"rationale":null}"#,
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
        let config = Config::default();
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
    fn validator_rejects_injury_conflict() {
        let mut config = Config::default();
        config.profile.injuries = vec!["legs and spine".to_string()];
        let candidate = LlmRecommendation {
            action: LlmAction::Recommend,
            movement_name: Some("squat".into()),
            reps: Some(5),
            sets: Some(1),
            weight_text: None,
            duration_sec: Some(30),
            primary_muscle: Some("legs".into()),
            muscles: Some(vec!["legs".into()]),
            equipment_used: None,
            safety_notes: Some("stop if pain appears".into()),
            rationale: Some("short movement".into()),
        };
        let err = validate_candidate(&config, &event(), candidate).unwrap_err();
        assert!(err.to_string().contains("limitations"));
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

        assert_eq!(store.queued_recommendation_count().unwrap(), QUEUE_TARGET);
        assert!(store.latest_open_recommendation().unwrap().is_none());
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

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
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

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::Hybrid);
        assert_eq!(generated.llm_count, 1);
        assert_eq!(generated.local_count, 4);
        assert_eq!(
            generated.notice.as_deref(),
            Some("Codex suggested 1; Svarog filled 4 locally.")
        );
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
        assert_eq!(movement_ids.len(), QUEUE_TARGET as usize);
        assert_eq!(muscles.len(), QUEUE_TARGET as usize);
    }

    #[test]
    fn empty_codex_queue_uses_local_fallback() {
        let store = test_store();
        let config = codex_config_returning(r#"{"recommendations":[]}"#);

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::LocalFallback);
        assert_eq!(generated.llm_count, 0);
        assert_eq!(generated.local_count, QUEUE_TARGET as usize);
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
            .find(|movement| movement.primary_muscle == "mobility")
            .unwrap();
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
            created_at: Utc::now(),
        };
        store.insert_queued_recommendation(&completed).unwrap();
        let completed = store
            .promote_next_queued_recommendation(Agent::Codex, None)
            .unwrap()
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        let queued = local_queue(&store, &Config::default(), &event(), 20, &[]).unwrap();

        assert_ne!(queued.first().unwrap().primary_muscle, "mobility");
        assert!(queued
            .iter()
            .any(|recommendation| recommendation.primary_muscle == "mobility"));
    }

    #[test]
    fn invalid_codex_candidates_are_skipped_before_local_top_up() {
        let store = test_store();
        let mut invalid = valid_wrist_candidate();
        invalid["movement_name"] = json!("unsafe duplicate");
        invalid["reps"] = json!(99);
        invalid["primary_muscle"] = json!("other");
        let message = json!({
            "recommendations": [valid_wrist_candidate(), invalid]
        })
        .to_string();
        let config = codex_config_returning(&message);

        let generated =
            generate_recommendation_queue(&store, &config, &test_paths(), QUEUE_TARGET).unwrap();

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
        assert_eq!(generated.llm_count, 1);
        assert_eq!(generated.local_count, 4);
        assert!(!generated
            .recommendations
            .iter()
            .any(|recommendation| recommendation.movement_name == "unsafe duplicate"));
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

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
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

        assert_eq!(generated.recommendations.len(), QUEUE_TARGET as usize);
        assert_eq!(generated.source, QueueGenerationSource::LocalFallback);
        assert_eq!(generated.notice.as_deref(), Some("Codex was unavailable."));
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
    }

    #[test]
    fn local_initial_profile_blocks_limited_body_parts() {
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
        let short_walk = movements
            .iter()
            .find(|movement| movement.id == "short_walk")
            .unwrap();

        assert_eq!(short_walk.status, MovementStatus::Blocked);
        assert!(notice.is_none());
    }
}
