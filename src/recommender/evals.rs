//! Live instruction-following evals for the OpenAI recommender prompts.
//!
//! Run with `OPENAI_API_KEY=… cargo test evals -- --ignored --nocapture`.
//! Optional: `SVAROG_EVAL_MODEL`, `SVAROG_EVAL_JUDGE_MODEL`, `SVAROG_EVAL_RUNS`,
//! `SVAROG_EVAL_FILTER` (scenario-name substring), `SVAROG_EVAL_CONCURRENCY`.

use super::*;
use crate::archetypes::{self, ArchetypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use tempfile::tempdir;

const DEFAULT_JUDGE_MODEL: &str = "gpt-6.1-sol";
const GOAL_THRESHOLD: f32 = 4.0;
const ARCHETYPE_THRESHOLD: f32 = 4.0;
const MIN_EQUIPMENT_SESSIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum Dimension {
    Preferences,
    Injuries,
    Goals,
    Equipment,
    Sides,
    Archetype,
    Combined,
}

struct Scenario {
    name: &'static str,
    dimension: Dimension,
    configure: fn(&mut Config),
    /// Expected normalized equipment as (kind, count); bodyweight is implied.
    expected_equipment: Option<&'static [(&'static str, u32)]>,
}

fn scenarios() -> Vec<Scenario> {
    fn s(
        name: &'static str,
        dimension: Dimension,
        configure: fn(&mut Config),
        expected_equipment: Option<&'static [(&'static str, u32)]>,
    ) -> Scenario {
        Scenario {
            name,
            dimension,
            configure,
            expected_equipment,
        }
    }
    use Dimension::*;
    vec![
        s(
            "pref_no_floor_no_jumping",
            Preferences,
            |c| {
                c.profile.exercise_preferences =
                    "no jumping, no bodyweight on the floor like push-ups, for example".into();
            },
            None,
        ),
        s(
            "pref_no_stretching",
            Preferences,
            |c| {
                c.profile.exercise_preferences = "no stretching, I want to actually work".into();
            },
            None,
        ),
        s(
            "pref_standing_only",
            Preferences,
            |c| {
                c.profile.exercise_preferences = "only standing movements, I'm in an office".into();
            },
            None,
        ),
        s(
            "pref_loves_kettlebells",
            Preferences,
            |c| {
                c.profile.equipment_text = "one 16kg kettlebell, resistance band".into();
                c.profile.exercise_preferences =
                    "I love kettlebell work, use it as much as possible".into();
            },
            Some(&[("kettlebell", 1), ("band", 1)]),
        ),
        s(
            "injury_lower_back",
            Injuries,
            |c| {
                c.profile.equipment_text = "two 12kg dumbbells".into();
                c.profile.injuries = vec!["lower back pain".into()];
            },
            Some(&[("dumbbell", 2)]),
        ),
        s(
            "injury_left_shoulder",
            Injuries,
            |c| {
                c.profile.equipment_text = "one 12kg dumbbell".into();
                c.profile.injuries = vec!["left shoulder impingement, no overhead pressing".into()];
            },
            Some(&[("dumbbell", 1)]),
        ),
        s(
            "injury_knees",
            Injuries,
            |c| {
                c.profile.injuries = vec!["bad knees, no deep squats or lunges".into()];
            },
            None,
        ),
        s(
            "injury_wrists",
            Injuries,
            |c| {
                c.profile.injuries = vec!["wrist pain, can't load the wrists in extension".into()];
            },
            None,
        ),
        s(
            "goal_mobility",
            Goals,
            |c| {
                c.profile.goals = vec!["mobility".into(), "flexibility".into()];
            },
            None,
        ),
        s(
            "goal_upper_strength",
            Goals,
            |c| {
                c.profile.equipment_text = "two 16kg dumbbells, pull-up bar".into();
                c.profile.goals = vec!["build upper-body strength".into()];
            },
            Some(&[("dumbbell", 2), ("pull_up_bar", 1)]),
        ),
        s(
            "goal_desk_posture",
            Goals,
            |c| {
                c.profile.equipment_text = "resistance band".into();
                c.profile.goals = vec!["better posture for desk work".into()];
            },
            Some(&[("band", 1)]),
        ),
        s(
            "goal_cardio",
            Goals,
            |c| {
                c.profile.goals = vec!["cardio endurance".into()];
            },
            None,
        ),
        s(
            "equip_bodyweight_only",
            Equipment,
            |c| {
                c.profile.equipment_text = "bodyweight only".into();
            },
            Some(&[]),
        ),
        s(
            "equip_two_kettlebells",
            Equipment,
            |c| {
                c.profile.equipment_text = "two 16kg kettlebells".into();
            },
            Some(&[("kettlebell", 2)]),
        ),
        s(
            "equip_dumbbell_band",
            Equipment,
            |c| {
                c.profile.equipment_text = "one 12kg dumbbell and a resistance band".into();
            },
            Some(&[("dumbbell", 1), ("band", 1)]),
        ),
        s(
            "equip_pull_up_bar",
            Equipment,
            |c| {
                c.profile.equipment_text = "a doorway pull-up bar".into();
            },
            Some(&[("pull_up_bar", 1)]),
        ),
        s(
            "sides_alternate",
            Sides,
            |c| {
                c.profile.equipment_text = "one 12kg dumbbell, one 16kg kettlebell".into();
                c.profile.alternate_sides = true;
            },
            Some(&[("dumbbell", 1), ("kettlebell", 1)]),
        ),
        s(
            "sides_both_at_once",
            Sides,
            |c| {
                c.profile.equipment_text = "one 12kg dumbbell, one 16kg kettlebell".into();
                c.profile.alternate_sides = false;
            },
            Some(&[("dumbbell", 1), ("kettlebell", 1)]),
        ),
        s(
            "arch_boxer",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Boxer,
            None,
        ),
        s(
            "arch_wrestler",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Wrestler,
            None,
        ),
        s(
            "arch_martial_artist",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::MartialArtist,
            None,
        ),
        s(
            "arch_bodybuilder",
            Archetype,
            |c| {
                c.profile.equipment_text = "two 12kg dumbbells".into();
                c.forge.archetype = ArchetypeId::Bodybuilder;
            },
            Some(&[("dumbbell", 2)]),
        ),
        s(
            "arch_runner",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Runner,
            None,
        ),
        s(
            "arch_athlete",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Athlete,
            None,
        ),
        s(
            "arch_gymnast",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Gymnast,
            None,
        ),
        s(
            "arch_yogi",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Yogi,
            None,
        ),
        s(
            "arch_mover",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Mover,
            None,
        ),
        s(
            "arch_thinker",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Thinker,
            None,
        ),
        s(
            "arch_lifer",
            Archetype,
            |c| c.forge.archetype = ArchetypeId::Lifer,
            None,
        ),
        s(
            "arch_custom_bruce_lee",
            Archetype,
            |c| {
                c.forge.archetype = ArchetypeId::Custom;
                c.forge.custom_archetype = Some("Bruce Lee".into());
            },
            None,
        ),
        s(
            "arch_custom_gibberish",
            Archetype,
            |c| {
                c.forge.archetype = ArchetypeId::Custom;
                c.forge.custom_archetype = Some("qwzx flurb".into());
            },
            None,
        ),
        s(
            "combined_real_user",
            Combined,
            |c| {
                c.profile.equipment_text = "one 16kg kettlebell, resistance band".into();
                c.profile.goals = vec!["stay strong".into(), "mobility".into()];
                c.profile.exercise_preferences =
                    "no jumping, no bodyweight on the floor like push-ups, for example".into();
                c.profile.injuries = vec!["occasional lower back tightness".into()];
                c.forge.archetype = ArchetypeId::Boxer;
            },
            Some(&[("kettlebell", 1), ("band", 1)]),
        ),
    ]
}

#[derive(Debug, Serialize)]
struct RunResult {
    scenario: &'static str,
    dimension: Dimension,
    run: usize,
    error: Option<String>,
    profile_notice: Option<String>,
    llm_count: usize,
    local_count: usize,
    picks: Vec<String>,
    rejected: Vec<String>,
    /// Hard failures: (check, detail).
    violations: Vec<(String, String)>,
    goal_score: Option<f32>,
    archetype_score: Option<f32>,
    archetype_metadata_share: Option<f32>,
}

impl RunResult {
    fn passed(&self) -> bool {
        self.error.is_none()
            && self.violations.is_empty()
            && self.goal_score.is_none_or(|s| s >= GOAL_THRESHOLD)
            && self
                .archetype_score
                .is_none_or(|s| s >= ARCHETYPE_THRESHOLD)
    }
}

fn eval_config(model: &str) -> Config {
    let mut config = Config::default();
    config.recommender.backend = RecommenderBackend::OpenaiEnv;
    config.recommender.local_fallback = false;
    config.recommender.show_llm_failures = true;
    config.recommender.timeout_ms = 300_000;
    config.recommender.openai.model = model.to_string();
    config
}

fn run_scenario(scenario: &Scenario, run: usize, model: &str, judge_model: &str) -> RunResult {
    let mut result = RunResult {
        scenario: scenario.name,
        dimension: scenario.dimension,
        run,
        error: None,
        profile_notice: None,
        llm_count: 0,
        local_count: 0,
        picks: Vec::new(),
        rejected: Vec::new(),
        violations: Vec::new(),
        goal_score: None,
        archetype_score: None,
        archetype_metadata_share: None,
    };
    if let Err(err) = run_scenario_inner(scenario, model, judge_model, &mut result) {
        result.error = Some(format!("{err:#}"));
    }
    result
}

fn run_scenario_inner(
    scenario: &Scenario,
    model: &str,
    judge_model: &str,
    result: &mut RunResult,
) -> Result<()> {
    let root = tempdir()?;
    let paths = Paths::from_root(root.path().join("svarog"));
    let store = Store::open(&root.path().join("eval.sqlite3"))?;
    store.seed_movements()?;
    let mut config = eval_config(model);
    (scenario.configure)(&mut config);
    store.save_user_profile(&config)?;

    let (movements, notice) = initial_exercise_profile(&store, &config, &paths);
    result.profile_notice = notice;
    store.replace_movement_pool(&movements)?;
    let equipment = store
        .exercise_filter::<Vec<EquipmentCapability>>()?
        .unwrap_or_default();
    if let Some(expected) = scenario.expected_equipment {
        check_equipment_profile(expected, &equipment, &mut result.violations);
    }

    let generation = generate_recommendation_queue(&store, &config, &paths, QUEUE_TARGET)?;
    result.llm_count = generation.llm_count;
    result.local_count = generation.local_count;
    result.rejected = generation.rejected;
    let recs = generation.recommendations;
    result.picks = recs
        .iter()
        .map(|rec| match rec.side {
            Some(side) => format!("{}[{side:?}]", rec.movement_id),
            None => rec.movement_id.clone(),
        })
        .collect();
    if recs.len() < QUEUE_TARGET as usize {
        result.violations.push((
            "queue_size".into(),
            format!(
                "{} valid of {QUEUE_TARGET}; rejected {:?}",
                recs.len(),
                result.rejected
            ),
        ));
    }

    let pool = store.movements()?;
    let by_id: BTreeMap<_, _> = pool.iter().map(|m| (m.id.as_str(), m)).collect();
    check_equipment_use(
        scenario,
        &config,
        &equipment,
        &recs,
        &by_id,
        &mut result.violations,
    );
    check_sides(&config, &recs, &by_id, &mut result.violations);
    result.archetype_metadata_share = Some(archetype_metadata_share(&config, &recs));

    let verdict = judge(&store, &config, &paths, judge_model, &recs)?;
    for item in verdict.items {
        if item.violates_preferences {
            result.violations.push((
                "preferences".into(),
                format!("{}: {}", item.exercise_id, item.reason),
            ));
        }
        if item.violates_injuries {
            result.violations.push((
                "injuries".into(),
                format!("{}: {}", item.exercise_id, item.reason),
            ));
        }
    }
    if verdict.preference_fit < 4 {
        result.violations.push((
            "preference_fit".into(),
            format!("positive preferences scored {}/5", verdict.preference_fit),
        ));
    }
    if !config.profile.goals.is_empty() && scenario.dimension != Dimension::Archetype {
        result.goal_score = Some(verdict.goal_alignment as f32);
    }
    if matches!(
        scenario.dimension,
        Dimension::Archetype | Dimension::Combined
    ) {
        result.archetype_score = Some(verdict.archetype_fit as f32);
    }
    Ok(())
}

fn check_equipment_profile(
    expected: &[(&str, u32)],
    actual: &[EquipmentCapability],
    violations: &mut Vec<(String, String)>,
) {
    let actual_kinds: BTreeMap<&str, u32> = actual
        .iter()
        .filter(|cap| cap.kind != "bodyweight")
        .map(|cap| (cap.kind.as_str(), cap.count))
        .collect();
    let expected_kinds: BTreeMap<&str, u32> = expected.iter().copied().collect();
    if actual_kinds != expected_kinds {
        violations.push((
            "equipment_profile".into(),
            format!("expected {expected_kinds:?}, normalized {actual_kinds:?}"),
        ));
    }
}

fn check_equipment_use(
    scenario: &Scenario,
    config: &Config,
    equipment: &[EquipmentCapability],
    recs: &[Recommendation],
    by_id: &BTreeMap<&str, &Movement>,
    violations: &mut Vec<(String, String)>,
) {
    let available: BTreeSet<&str> = equipment.iter().map(|cap| cap.kind.as_str()).collect();
    let mut equipment_sessions = 0;
    for rec in recs {
        let Some(movement) = by_id.get(rec.movement_id.as_str()) else {
            violations.push(("catalog".into(), format!("{} not in pool", rec.movement_id)));
            continue;
        };
        let missing: Vec<_> = movement
            .equipment
            .iter()
            .filter(|kind| !available.contains(kind.as_str()))
            .collect();
        if !missing.is_empty() {
            violations.push((
                "equipment_available".into(),
                format!("{} needs {missing:?}", rec.movement_id),
            ));
        }
        let uses_equipment = exercise_catalog::equipment_requirements_for_movement(&movement.id)
            .unwrap_or_default()
            .iter()
            .any(|requirement| requirement.kind != "bodyweight");
        if uses_equipment {
            equipment_sessions += 1;
        }
    }
    // Only enforce "don't default to bodyweight" when nothing else steers away from equipment.
    let neutral = config.profile.exercise_preferences == "automatic"
        && config.profile.injuries.is_empty()
        && matches!(scenario.dimension, Dimension::Equipment | Dimension::Sides);
    // Expect at least 4 of every 10 sessions to use equipment (the prompt targets 5).
    if neutral
        && available.len() > 1
        && equipment_sessions * 10 < MIN_EQUIPMENT_SESSIONS * recs.len()
    {
        violations.push((
            "equipment_share".into(),
            format!(
                "only {equipment_sessions} of {} sessions use equipment",
                recs.len()
            ),
        ));
    }
}

fn check_sides(
    config: &Config,
    recs: &[Recommendation],
    by_id: &BTreeMap<&str, &Movement>,
    violations: &mut Vec<(String, String)>,
) {
    for (index, rec) in recs.iter().enumerate() {
        let unilateral = by_id
            .get(rec.movement_id.as_str())
            .is_some_and(|m| m.sidedness == MovementSidedness::Unilateral);
        let single = matches!(
            rec.side,
            Some(RecommendationSide::Left | RecommendationSide::Right)
        );
        if !config.profile.alternate_sides {
            if single {
                violations.push(("sides".into(), format!("{} split by side", rec.movement_id)));
            }
            continue;
        }
        if unilateral && !single {
            violations.push((
                "sides".into(),
                format!("{} unilateral without left/right", rec.movement_id),
            ));
        }
        if single {
            let paired = [index.checked_sub(1), Some(index + 1)]
                .into_iter()
                .flatten()
                .filter_map(|i| recs.get(i))
                .any(|other| is_opposite_side_pair(other, rec));
            if !paired {
                violations.push((
                    "sides".into(),
                    format!(
                        "{} {:?} has no adjacent opposite side",
                        rec.movement_id, rec.side
                    ),
                ));
            }
        }
    }
}

/// Share of picks matching the archetype's preferred catalog category (deterministic signal).
fn archetype_metadata_share(config: &Config, recs: &[Recommendation]) -> f32 {
    if recs.is_empty() {
        return 0.0;
    }
    let archetype = archetypes::get(config.forge.archetype);
    let matching = recs
        .iter()
        .filter_map(|rec| exercise_catalog::find(&rec.movement_id))
        .filter(|entry| {
            archetype
                .preferred_categories
                .contains(&entry.category.as_str())
                || entry
                    .primary_muscles
                    .iter()
                    .any(|m| archetype.preferred_muscles.contains(&m.as_str()))
        })
        .count();
    matching as f32 / recs.len() as f32
}

#[derive(Debug, Deserialize)]
struct JudgeVerdict {
    items: Vec<JudgeItem>,
    goal_alignment: u8,
    archetype_fit: u8,
    preference_fit: u8,
}

#[derive(Debug, Deserialize)]
struct JudgeItem {
    exercise_id: String,
    violates_preferences: bool,
    violates_injuries: bool,
    reason: String,
}

fn judge(
    store: &Store,
    config: &Config,
    paths: &Paths,
    judge_model: &str,
    recs: &[Recommendation],
) -> Result<JudgeVerdict> {
    let archetype = archetypes::get(config.forge.archetype);
    let archetype_text = match (config.forge.archetype, &config.forge.custom_archetype) {
        (ArchetypeId::Custom, Some(name)) => format!(
            "Custom: {name:?} (if not a recognizable person/style, it should behave as Athlete: {})",
            archetype.description
        ),
        _ => format!("{}: {}", archetype.name, archetype.description),
    };
    let movements: Vec<_> = recs
        .iter()
        .map(|rec| {
            let entry = exercise_catalog::find(&rec.movement_id);
            let instructions = entry
                .map(|e| e.instructions.join(" "))
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect::<String>();
            json!({
                "exercise_id": rec.movement_id,
                "name": exercise_catalog::display_name(&rec.movement_id),
                "category": entry.map(|e| e.category.as_str()),
                "equipment": entry.and_then(|e| e.equipment.as_deref()),
                "force": entry.and_then(|e| e.force.as_deref()),
                "primary_muscles": entry.map(|e| &e.primary_muscles),
                "instructions": instructions,
                "side": rec.side,
                "reps": rec.reps,
                "weight_kg": rec.weight_kg,
            })
        })
        .collect();
    let prompt = format!(
        "You are a strict evaluator of an exercise recommender's instruction following.\n\
         The user configured these settings:\n{settings}\n\n\
         The recommender produced this queue of short desk-break exercise sessions:\n{movements}\n\n\
         For EACH exercise, decide:\n\
         - violates_preferences: true only if the exercise is something the user's preferences \
         exclude or dislike, including the whole class of movements the user describes (an example \
         like \"push-ups\" stands for the described class), judged from the name, category and \
         instructions. Positive preferences (\"I love X\") never make a single exercise a \
         violation; score them in preference_fit instead. Ignore side labels; they are checked \
         separately. If preferences are \"automatic\" this is always false.\n\
         - violates_injuries: true if it plausibly loads or aggravates a stated injury/limitation \
         at the prescribed load. Conservative, injury-friendly variants are fine. If there are no \
         injuries this is always false.\n\
         - reason: one short sentence (empty if neither is violated).\n\
         Then score the whole batch 1-5:\n\
         - preference_fit: how well the batch reflects positive preferences (5 if there are none).\n\
         - goal_alignment: how well the batch serves the stated goals (5 = clearly most sessions \
         serve them).\n\
         - archetype_fit: how well the batch reflects the Forge archetype's long-term direction \
         within the user's equipment and constraints (5 = clear, appropriate bias).\n",
        settings = serde_json::to_string_pretty(&json!({
            "goals": config.profile.goals,
            "equipment": config.profile.equipment_text,
            "exercise_preferences": config.profile.exercise_preferences,
            "injuries_and_limitations": config.profile.injuries,
            "alternate_sides_for_one_sided_exercises": config.profile.alternate_sides,
            "forge_archetype": archetype_text,
        }))?,
        movements = serde_json::to_string_pretty(&movements)?,
    );
    let body = json!({
        "model": judge_model,
        "reasoning": { "effort": "medium" },
        "input": [{ "role": "user", "content": prompt }],
        "text": { "format": {
            "type": "json_schema",
            "name": "svarog_eval_verdict",
            "strict": true,
            "schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["items", "goal_alignment", "archetype_fit", "preference_fit"],
                "properties": {
                    "items": { "type": "array", "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["exercise_id", "violates_preferences", "violates_injuries", "reason"],
                        "properties": {
                            "exercise_id": { "type": "string" },
                            "violates_preferences": { "type": "boolean" },
                            "violates_injuries": { "type": "boolean" },
                            "reason": { "type": "string" }
                        }
                    }},
                    "goal_alignment": { "type": "integer", "minimum": 1, "maximum": 5 },
                    "archetype_fit": { "type": "integer", "minimum": 1, "maximum": 5 },
                    "preference_fit": { "type": "integer", "minimum": 1, "maximum": 5 }
                }
            }
        }}
    });
    call_openai_json(store, config, paths, body).context("judging recommendation queue")
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[test]
#[ignore = "calls the OpenAI API; run with OPENAI_API_KEY and --ignored"]
fn evals() {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("OPENAI_API_KEY is not set; skipping recommender evals");
        return;
    }
    let model = env_or(
        "SVAROG_EVAL_MODEL",
        &Config::default().recommender.openai.model,
    );
    let judge_model = env_or("SVAROG_EVAL_JUDGE_MODEL", DEFAULT_JUDGE_MODEL);
    let runs: usize = env_or("SVAROG_EVAL_RUNS", "3").parse().unwrap_or(3);
    let concurrency: usize = env_or("SVAROG_EVAL_CONCURRENCY", "8").parse().unwrap_or(8);
    let filter = std::env::var("SVAROG_EVAL_FILTER").unwrap_or_default();

    let scenarios: Vec<_> = scenarios()
        .into_iter()
        .filter(|scenario| filter.split(',').any(|f| scenario.name.contains(f.trim())))
        .collect();
    let jobs: Vec<(usize, usize)> = (0..scenarios.len())
        .flat_map(|index| (0..runs).map(move |run| (index, run)))
        .collect();
    let next = Mutex::new(0usize);
    let results = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..concurrency.max(1) {
            scope.spawn(|| loop {
                let job = {
                    let mut next = next.lock().unwrap();
                    let job = jobs.get(*next).copied();
                    *next += 1;
                    job
                };
                let Some((index, run)) = job else { break };
                let result = run_scenario(&scenarios[index], run, &model, &judge_model);
                eprintln!(
                    "{} {}#{}",
                    if result.passed() { "pass" } else { "FAIL" },
                    result.scenario,
                    result.run
                );
                results.lock().unwrap().push(result);
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| (r.dimension, r.scenario, r.run));

    println!("\nmodel={model} judge={judge_model} runs={runs}\n");
    println!(
        "{:<26} {:>6} {:>6} {:>6} {:>6}  failures",
        "scenario", "pass", "goal", "arch", "meta"
    );
    let mut by_scenario: BTreeMap<(Dimension, &str), Vec<&RunResult>> = BTreeMap::new();
    for result in &results {
        by_scenario
            .entry((result.dimension, result.scenario))
            .or_default()
            .push(result);
    }
    let mut by_dimension: BTreeMap<Dimension, (usize, usize)> = BTreeMap::new();
    for ((dimension, scenario), runs) in &by_scenario {
        let passed = runs.iter().filter(|r| r.passed()).count();
        let entry = by_dimension.entry(*dimension).or_default();
        entry.0 += passed;
        entry.1 += runs.len();
        let mean = |values: Vec<f32>| {
            if values.is_empty() {
                "-".to_string()
            } else {
                format!("{:.1}", values.iter().sum::<f32>() / values.len() as f32)
            }
        };
        let mut failures: BTreeMap<String, usize> = BTreeMap::new();
        for run in runs {
            if let Some(error) = &run.error {
                *failures.entry(format!("error: {error}")).or_default() += 1;
            }
            for (check, detail) in &run.violations {
                *failures.entry(format!("{check}: {detail}")).or_default() += 1;
            }
        }
        println!(
            "{:<26} {:>3}/{:<2} {:>6} {:>6} {:>6}",
            scenario,
            passed,
            runs.len(),
            mean(runs.iter().filter_map(|r| r.goal_score).collect()),
            mean(runs.iter().filter_map(|r| r.archetype_score).collect()),
            mean(
                runs.iter()
                    .filter_map(|r| r.archetype_metadata_share)
                    .collect()
            ),
        );
        for (failure, count) in failures {
            println!("    {count}x {failure}");
        }
    }
    println!("\nby dimension:");
    for (dimension, (passed, total)) in &by_dimension {
        println!("  {dimension:?}: {passed}/{total}");
    }

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/evals");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{}.json", Utc::now().format("%Y%m%dT%H%M%S")));
    let report = json!({
        "model": model,
        "judge_model": judge_model,
        "runs": runs,
        "results": results,
    });
    if std::fs::write(&file, serde_json::to_string_pretty(&report).unwrap()).is_ok() {
        println!("\nreport: {}", file.display());
    }

    let failed = results.iter().filter(|r| !r.passed()).count();
    assert_eq!(failed, 0, "{failed} of {} eval runs failed", results.len());
}
