use crate::config::{self, Config, RuntimeEnv, UnitSystem};
use crate::daemon;
use crate::hooks;
use crate::models::{Agent, AppStateKind, IncomingEvent, MovementStatus, SetStatus};
use crate::recommender;
use crate::session;
use crate::stop;
use crate::storage::Store;
use crate::tui;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "svarog")]
#[command(version)]
#[command(about = "Turn AI-agent waiting time into tiny, adaptive workouts.")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Setup {
        #[arg(long)]
        dev: bool,
        #[arg(long)]
        dry_run: bool,
        /// Erase all user data and restart onboarding.
        #[arg(long, conflicts_with = "dry_run")]
        reset: bool,
    },
    #[command(hide = true)]
    Init,
    #[command(hide = true)]
    Calibrate,
    /// Open a coding agent and Svarog together in an optional tmux session.
    Session {
        agent: Agent,
    },
    /// Open an isolated project-local Svarog environment.
    Demo {
        /// Compatibility flag; demo always confirms a reset and reruns onboarding.
        #[arg(long)]
        remove_data: bool,
    },
    /// Run the interactive Svarog dashboard and collector.
    Run,
    /// Stop Svarog runtimes and Svarog-created tmux sessions.
    Stop,
    #[command(hide = true)]
    CodexHook,
    #[command(hide = true)]
    Daemon,
    Status,
    #[command(hide = true)]
    Hook {
        agent: Agent,
        #[arg(long)]
        install: bool,
    },
    #[command(hide = true)]
    Event {
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        event: String,
        #[arg(long, alias = "expected-duration-sec")]
        duration: Option<u32>,
        #[arg(long)]
        project: Option<String>,
    },
    Start,
    Done,
    Skip,
    Pain,
    /// List or restore exercises removed from recommendations.
    Exercises {
        #[command(subcommand)]
        command: ExerciseCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ExerciseCommand {
    /// List removed canonical exercise IDs.
    Removed,
    /// Restore one removed exercise ID.
    Restore { exercise_id: String },
    /// Restore every removed exercise.
    RestoreAll,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let env = RuntimeEnv::load()?;

    match cli.command {
        None => launch(&env).await,
        Some(Command::Setup {
            dev,
            dry_run,
            reset,
        }) => {
            let setup_env = RuntimeEnv::load_with_options(dev, dry_run)?;
            setup(&setup_env, reset)
        }
        Some(Command::Init) => init(&env),
        Some(Command::Calibrate) => calibrate(&env),
        Some(Command::Session { agent }) => session::run(agent, &env),
        Some(Command::Demo { remove_data }) => run_demo(remove_data).await,
        Some(Command::Run) => launch(&env).await,
        Some(Command::Stop) => stop::run(&env),
        Some(Command::CodexHook) => hooks::ingest_codex(&env).await,
        Some(Command::Daemon) => daemon::run().await,
        Some(Command::Status) => status(&env),
        Some(Command::Hook { agent, install }) => {
            if install {
                if agent == Agent::Codex {
                    let path = hooks::install_global_codex(&env)?;
                    println!("Installed Codex hook config: {}", path.display());
                } else {
                    let path = hooks::install(&env, agent)?;
                    println!("Installed hook script: {}", path.display());
                }
            } else {
                hooks::print(agent);
            }
            Ok(())
        }
        Some(Command::Event {
            agent,
            event,
            duration,
            project,
        }) => {
            let payload = IncomingEvent {
                agent,
                event,
                expected_duration_sec: duration,
                duration_sec: None,
                project,
            };
            let response = daemon::process_event(&env, payload)?;
            if let Some(notice) = response.notice.as_deref() {
                println!("{notice}");
            }
            if let Some(rec) = response.recommendation {
                println!("{}: {} {}", rec.agent, rec.reps, rec.display_name());
            } else {
                println!("no recommendation");
            }
            Ok(())
        }
        Some(Command::Start) => action(&env, SetStatus::Started),
        Some(Command::Done) => action(&env, SetStatus::Done),
        Some(Command::Skip) => action(&env, SetStatus::Skipped),
        Some(Command::Pain) => action(&env, SetStatus::Pain),
        Some(Command::Exercises { command }) => exercises(&env, command),
    }
}

fn setup(env: &RuntimeEnv, reset: bool) -> Result<()> {
    if env.dry_run {
        return setup_dry_run(env);
    }
    let paths = &env.paths;
    let existing_config = paths.config_file.exists();
    let mut config = if reset {
        confirm_full_reset()?;
        daemon::ensure_tui_available(env)?;
        reset_user_data(env)?;
        println!();
        Config::default()
    } else if existing_config {
        config::load_or_default(paths)?
    } else {
        Config::default()
    };
    print_setup_intro(env);

    if existing_config && !reset {
        match prompt_existing_profile_action()? {
            ExistingProfileAction::Continue => {}
            ExistingProfileAction::DestroyProfile => {
                config = Config::default();
            }
            ExistingProfileAction::DestroyAll => {
                daemon::ensure_tui_available(env)?;
                reset_user_data(env)?;
                config = Config::default();
            }
        }
        println!();
    }

    collect_profile(&mut config, paths, true)?;

    finish_setup(env, &config)
}

fn setup_pending(env: &RuntimeEnv) -> Result<()> {
    let paths = &env.paths;
    let mut config = config::load_or_default(paths)?;
    let pending = config.onboarding.pending_steps();
    if paths.config_file.exists() {
        if pending.is_empty() {
            println!("{}", ember_bold("🔥 Repairing Svarog setup"));
            println!("{} {}", muted("Environment:"), text(env.mode_label()));
            println!(
                "{}",
                muted("Your existing answers and history will be preserved.")
            );
        } else {
            println!("{}", ember_bold("🔥 Svarog onboarding update"));
            println!("{} {}", muted("Environment:"), text(env.mode_label()));
            println!(
                "{}",
                muted(format!(
                    "{} new setup question(s). Existing answers and history will be preserved.",
                    pending.len()
                ))
            );
        }
        println!();
    } else {
        print_setup_intro(env);
    }

    collect_profile(&mut config, paths, false)?;
    finish_setup(env, &config)
}

fn reset_user_data(env: &RuntimeEnv) -> Result<()> {
    let config = Config::default();
    config::save(&env.paths, &config)?;
    let store = Store::open(&env.paths.database_file)?;
    store.reset_all_data()?;
    println!("All Svarog user data was reset. Starting onboarding from the beginning.");
    Ok(())
}

fn finish_setup(env: &RuntimeEnv, config: &Config) -> Result<()> {
    let paths = &env.paths;
    config::save(paths, config)?;
    let store = Store::open(&paths.database_file)?;
    store.clear_queued_recommendations()?;
    store.clear_exercise_exclusions()?;
    store.save_user_profile(config)?;
    println!();
    println!(
        "{} {}",
        text("Checking recommendation engine..."),
        muted(config.recommender.backend.label())
    );
    let mut recommender_notices = 0;
    let (movements, profile_notice) =
        recommender::initial_exercise_profile(&store, config, &env.paths);
    store.replace_movement_pool(&movements)?;
    if let Some(notice) = profile_notice {
        recommender_notices += 1;
        print_recommender_notice(config, env, &notice);
    }
    if let Some(notice) = recommender::fill_recommendation_queue(&store, config, &env.paths)? {
        recommender_notices += 1;
        print_recommender_notice(config, env, &notice);
    }
    if recommender_notices == 0 {
        println!("{} {}", ember("✓"), text("Recommendation engine ready"));
    }

    println!();
    println!("{}", text("Installing Codex integration..."));
    hooks::install_global_codex(env)?;
    println!("{} {}", ember("✓"), text("Hook installed"));
    println!();
    print_setup_summary(config);
    Ok(())
}

fn print_setup_intro(env: &RuntimeEnv) {
    println!("{}", ember_bold("🔥 Svarog setup"));
    println!("{} {}", muted("Environment:"), text(env.mode_label()));
    println!();
    println!(
        "{}",
        muted("Svarog gives you short forging sessions while your AI agent works.")
    );
    println!();
    println!(
        "{}",
        muted("This will create your profile, configure your exercises,")
    );
    println!("{}", muted("and connect to your coding agent."));
    println!();
    println!("{}", text_bold("Press Enter to accept defaults."));
    println!();
}

async fn run_tui(env: &RuntimeEnv) -> Result<()> {
    daemon::refresh_exercise_pool(env)?;
    let collector = daemon::Collector::start(env).await?;
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tui_shutdown = std::sync::Arc::clone(&shutdown);
    let tui_env = env.clone();
    let tui_task = tokio::task::spawn_blocking(move || tui::run(&tui_env, tui_shutdown));
    #[cfg(unix)]
    let signal_task = {
        let shutdown = std::sync::Arc::clone(&shutdown);
        tokio::spawn(async move {
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                return;
            };
            if terminate.recv().await.is_some() {
                shutdown.store(true, std::sync::atomic::Ordering::Release);
            }
        })
    };
    let tui_result = tui_task.await.context("joining Svarog TUI")?;
    #[cfg(unix)]
    signal_task.abort();
    let shutdown_result = collector.shutdown().await;
    tui_result?;
    shutdown_result
}

async fn launch(env: &RuntimeEnv) -> Result<()> {
    if production_needs_setup(env)? {
        setup_pending(env)?;
        wait_for_tui("Press Enter to open the Svarog TUI: ")?;
    }
    run_tui(env).await
}

fn production_needs_setup(env: &RuntimeEnv) -> Result<bool> {
    if !env.paths.config_file.exists() {
        return Ok(true);
    }
    let config = config::load_or_default(&env.paths)?;
    Ok(!config.onboarding.is_complete()
        || !env.paths.database_file.exists()
        || !env.codex_home.join("hooks.json").exists())
}

async fn run_demo(_remove_data: bool) -> Result<()> {
    let env = RuntimeEnv::load_demo()?;
    let root = demo_root(&env)?;
    let reset = if root.exists() {
        daemon::ensure_tui_available(&env)?;
        prompt_demo_existing_action(&root)? == DemoExistingAction::Reset
    } else {
        true
    };

    if reset {
        if root.exists() {
            remove_demo_data(&root)?;
            println!("Removed demo data: {}", root.display());
            println!("This cannot be recovered. Production data was not changed.");
            println!();
        }
        setup(&env, false)?;
        wait_for_tui("Press Enter to open the demo TUI: ")?;
    } else if production_needs_setup(&env)? {
        setup_pending(&env)?;
        wait_for_tui("Press Enter to open the demo TUI: ")?;
    }

    run_tui(&env).await
}

fn demo_root(env: &RuntimeEnv) -> Result<PathBuf> {
    let Some(root) = env.paths.config_dir.parent() else {
        bail!("demo sandbox has no parent directory");
    };
    if root.file_name().and_then(|name| name.to_str()) != Some(".svarog-dev")
        || env.codex_home.parent() != Some(root)
    {
        bail!(
            "refusing to use an invalid demo sandbox: {}",
            root.display()
        );
    }
    Ok(root.to_path_buf())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoExistingAction {
    Resume,
    Reset,
}

fn prompt_demo_existing_action(root: &Path) -> Result<DemoExistingAction> {
    println!("Existing demo data found:");
    println!("  {}", root.display());
    println!("Production Svarog data and hooks will not be changed.");
    print!(
        "Press Enter to continue, or type \"remove\" to remove all current demo data and start over: "
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_demo_existing_action(input.trim())
}

fn parse_demo_existing_action(value: &str) -> Result<DemoExistingAction> {
    match value {
        "" => Ok(DemoExistingAction::Resume),
        "remove" => Ok(DemoExistingAction::Reset),
        _ => bail!(
            "demo launch cancelled; press Enter to continue or type exactly \"remove\" to remove all current demo data and start over"
        ),
    }
}

fn remove_demo_data(root: &Path) -> Result<()> {
    if root.file_name().and_then(|name| name.to_str()) != Some(".svarog-dev") {
        bail!(
            "refusing to remove an invalid demo sandbox: {}",
            root.display()
        );
    }
    fs::remove_dir_all(root).with_context(|| format!("removing {}", root.display()))
}

fn wait_for_tui(prompt: &str) -> Result<()> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(())
}

fn setup_dry_run(env: &RuntimeEnv) -> Result<()> {
    println!("{}", ember_bold("🔥 Svarog setup dry run"));
    println!("{} {}", muted("Environment:"), text(env.mode_label()));
    println!();
    println!("{}", muted("Would create profile with default answers."));
    println!(
        "{} {}",
        muted("Would write config:"),
        text(env.paths.config_file.display())
    );
    println!(
        "{} {}",
        muted("Would write database:"),
        text(env.paths.database_file.display())
    );
    println!(
        "{} {}",
        muted("Would install Svarog hook script:"),
        text(
            env.paths
                .config_dir
                .join("hooks")
                .join("codex-event.sh")
                .display()
        )
    );
    println!(
        "{} {}",
        muted("Would install Codex hook config:"),
        text(env.codex_home.join("hooks.json").display())
    );
    println!(
        "{} {}",
        muted("Would use collector address:"),
        text(env.daemon_addr)
    );
    println!("{} {}", muted("Would start Svarog:"), text("no"));
    Ok(())
}

fn init(env: &RuntimeEnv) -> Result<()> {
    let paths = &env.paths;
    let mut config = Config::default();
    println!("Svarog init");
    println!("Press enter to accept defaults.");

    collect_profile(&mut config, paths, true)?;

    config::save(paths, &config)?;
    let store = Store::open(&paths.database_file)?;
    store.save_user_profile(&config)?;
    let equipment =
        crate::exercise_catalog::locally_resolved_equipment(&config.profile.equipment_text);
    store.replace_movement_pool(&crate::exercise_catalog::movements_for_equipment(
        &equipment,
    ))?;
    println!("Config: {}", paths.config_file.display());
    println!("Database: {}", paths.database_file.display());
    Ok(())
}

fn collect_profile(config: &mut Config, paths: &config::Paths, all: bool) -> Result<()> {
    let collecting_measurements = all
        || !config.onboarding.is_completed(config::STEP_HEIGHT)
        || !config.onboarding.is_completed(config::STEP_WEIGHT);
    if collecting_measurements {
        let use_metric = prompt_bool(
            "Use metric units? (n = imperial)",
            config.profile.unit_system == UnitSystem::Metric,
        )?;
        config.profile.unit_system = unit_system_from_metric(use_metric);
        config::save(paths, config)?;
    }

    onboarding_step(config, paths, config::STEP_HEIGHT, all, |config| {
        config.profile.height_cm = match config.profile.unit_system {
            UnitSystem::Metric => prompt_parse("Height cm", config.profile.height_cm)?,
            UnitSystem::Imperial => prompt_imperial_height(config.profile.height_cm)?,
        };
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_WEIGHT, all, |config| {
        config.profile.weight_kg = match config.profile.unit_system {
            UnitSystem::Metric => prompt_parse("Weight kg", config.profile.weight_kg)?,
            UnitSystem::Imperial => prompt_imperial_weight(config.profile.weight_kg)?,
        };
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_AGE, all, |config| {
        config.profile.age = prompt_parse("Age", config.profile.age)?;
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_GOALS, all, |config| {
        config.profile.goals = prompt_list("Goals", &config.profile.goals)?;
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_EQUIPMENT, all, |config| {
        config.profile.equipment_text = prompt_multiline_string(
            "What equipment do you have near your desk?",
            &config.profile.equipment_text,
        )?;
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_WORK_SETUP, all, |config| {
        let sitting = prompt_bool(
            "Do you usually sit while agents run?\n(n = standing)",
            config.profile.work_setup != "standing",
        )?;
        config.profile.work_setup = if sitting {
            "sitting".to_string()
        } else {
            "standing".to_string()
        };
        Ok(())
    })?;
    onboarding_step(
        config,
        paths,
        config::STEP_ARM_AVAILABILITY,
        all,
        |config| {
            let alternate_arms = prompt_bool(
                "Alternate arms between forging sessions?",
                config.profile.one_hand_available,
            )?;
            config.profile.one_hand_available = alternate_arms;
            config.profile.two_hand_available = !alternate_arms;
            Ok(())
        },
    )?;
    onboarding_step(
        config,
        paths,
        config::STEP_CAUTIOUS_BODY_PARTS,
        all,
        |config| {
            config.profile.cautious_body_parts =
                prompt_list("Cautious body parts", &config.profile.cautious_body_parts)?;
            Ok(())
        },
    )?;
    onboarding_step(config, paths, config::STEP_INJURIES, all, |config| {
        config.profile.injuries =
            prompt_list("Injuries or hard limitations", &config.profile.injuries)?;
        Ok(())
    })?;
    onboarding_step(config, paths, config::STEP_ARCHETYPE, all, |config| {
        config.forge = tui::select_archetype(&config.forge)?;
        Ok(())
    })?;
    onboarding_step(
        config,
        paths,
        config::STEP_DESKTOP_NOTIFICATIONS,
        all,
        |config| {
            config.preferences.desktop_notifications = prompt_bool(
                "Notify you when your next exercise is ready?",
                config.preferences.desktop_notifications,
            )?;
            Ok(())
        },
    )?;
    onboarding_step(config, paths, config::STEP_CODEX_COMMAND, all, |config| {
        config.agents.codex_command = prompt_string("Codex command", &config.agents.codex_command)?;
        Ok(())
    })?;
    onboarding_step(
        config,
        paths,
        config::STEP_EXERCISE_PREFERENCES,
        all,
        |config| {
            config.profile.exercise_preferences =
                prompt_string(
                    "Exercise preferences (leave automatic, or enter preferences such as \"posture and stretching\" or \"no jumping\")",
                    &config.profile.exercise_preferences,
                )?;
            Ok(())
        },
    )?;
    Ok(())
}

fn onboarding_step<F>(
    config: &mut Config,
    paths: &config::Paths,
    step: &'static str,
    all: bool,
    prompt: F,
) -> Result<()>
where
    F: FnOnce(&mut Config) -> Result<()>,
{
    if !all && config.onboarding.is_completed(step) {
        return Ok(());
    }
    prompt(config)?;
    config.onboarding.mark_completed(step);
    config::save(paths, config)
}

fn print_setup_summary(config: &Config) {
    println!("{}", ember("══════════════════════════════════"));
    println!();
    println!("{}", ember_bold("Svarog is ready."));
    println!();
    println!(
        "{}",
        text("You'll receive forging sessions while `svarog run` is open and Codex works.")
    );
    println!();
    println!("{}", ember("Current settings"));
    println!();
    println!("{}", muted("Goal:"));
    println!("{}", text(config.profile.goals.join(", ")));
    println!();
    println!("{}", muted("Equipment:"));
    println!("{}", text(&config.profile.equipment_text));
    println!();
    println!("{}", muted("Forge archetype:"));
    println!(
        "{}",
        text(crate::archetypes::display_name(
            config.forge.archetype,
            config.forge.custom_archetype.as_deref(),
        ))
    );
    println!();
    println!("{}", muted("Notifications:"));
    println!(
        "{}",
        text(if config.preferences.desktop_notifications {
            "enabled"
        } else {
            "disabled"
        })
    );
    println!();
    println!("{}", muted("Exercise selection:"));
    println!("{}", text(&config.profile.exercise_preferences));
    println!();
    println!("{}", muted("Recommendation engine:"));
    println!("{}", text(config.recommender.backend.label()));
    println!();
    println!("{}", muted("Agent:"));
    println!("{}", text("Codex"));
    println!();
    println!(
        "{}",
        muted("Codex may ask once to trust the Svarog hook. Use /hooks if prompted.")
    );
    println!();
    println!("{}", ember("Happy forging."));
}

fn print_recommender_notice(config: &Config, env: &RuntimeEnv, notice: &str) {
    println!(
        "{} {}",
        muted("Recommender check:"),
        text(config.recommender.backend.label())
    );
    println!("{}", muted(notice));
    println!(
        "{} {}",
        muted("Config:"),
        text(env.paths.config_file.display())
    );
}

fn calibrate(env: &RuntimeEnv) -> Result<()> {
    let paths = &env.paths;
    paths.ensure()?;
    let store = Store::open(&paths.database_file)?;
    store.seed_movements()?;

    println!("Svarog calibration");
    println!("Answer e=easy, g=good, h=hard, p=pain, i=impossible.");

    for mut movement in store.movements()? {
        println!();
        println!("Test: {}", movement.name);
        println!("Target: {} reps", movement.base_reps.min(5));
        let answer = prompt_string("[e/g/h/p/i]", "g")?;
        movement.status = match answer.trim() {
            "e" | "g" => MovementStatus::Allowed,
            "h" => MovementStatus::Caution,
            "p" | "i" => MovementStatus::Blocked,
            _ => MovementStatus::Caution,
        };
        store.upsert_movement(&movement)?;
    }

    println!("Calibration saved.");
    print_whitelist(&store)?;
    Ok(())
}

fn print_whitelist(store: &Store) -> Result<()> {
    let movements = store.movements()?;
    println!();
    for (label, status) in [
        ("allowed", MovementStatus::Allowed),
        ("caution", MovementStatus::Caution),
        ("blocked", MovementStatus::Blocked),
    ] {
        println!("[{label}]");
        for movement in movements
            .iter()
            .filter(|movement| movement.status == status)
        {
            println!("{} = true", movement.id);
        }
        println!();
    }
    Ok(())
}

fn status(env: &RuntimeEnv) -> Result<()> {
    let paths = &env.paths;
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("Environment: {}", env.mode_label());
    println!("Collector: {} (runs with `svarog run`)", env.daemon_addr);
    println!("Codex: {}", env.codex_home.display());
    let config_exists = paths.config_file.exists();
    let db_exists = paths.database_file.exists();
    println!(
        "Config: {}",
        if config_exists {
            paths.config_file.display().to_string()
        } else {
            "missing".into()
        }
    );
    println!(
        "Database: {}",
        if db_exists {
            paths.database_file.display().to_string()
        } else {
            "missing".into()
        }
    );

    if db_exists {
        let store = Store::open(&paths.database_file)?;
        let (sets, reps, breaks) = store.stats_today()?;
        println!("Today: {sets} sets, {reps} reps, {breaks} breaks");
        println!("Queued: {}", store.queued_recommendation_count()?);
        if let Some(rec) = store.latest_open_recommendation()? {
            println!("Current: {} {}", rec.reps, rec.display_name());
        }
        let state = store.state()?;
        println!("State: {}", state.kind.as_str());
        println!("Updated: {}", state.updated_at.format("%Y-%m-%d %H:%M"));
        if let Some(id) = state.current_recommendation_id {
            println!("Recommendation: #{id}");
        }
        if let (Some(muscle), Some(until)) = (state.cooldown_muscle, state.cooldown_until) {
            println!("Cooldown: avoid {muscle} until {}", until.format("%H:%M"));
        }
    }
    Ok(())
}

pub fn action(env: &RuntimeEnv, status: SetStatus) -> Result<()> {
    action_with_options(env, status, None, false, true)
}

pub fn tui_action(env: &RuntimeEnv, status: SetStatus) -> Result<()> {
    action_with_options(env, status, None, false, false)
}

pub fn tui_action_with_reps(env: &RuntimeEnv, status: SetStatus, reps: u32) -> Result<()> {
    action_with_options(env, status, Some(reps), false, false)
}

pub fn tui_action_skip_fatigued(env: &RuntimeEnv) -> Result<()> {
    action_with_options(env, SetStatus::Skipped, None, true, false)
}

pub fn tui_action_remove_exercise(env: &RuntimeEnv) -> Result<()> {
    let store = Store::open(&env.paths.database_file)?;
    let Some(rec) = store.latest_open_recommendation()? else {
        bail!("no active recommendation");
    };
    store.record_set(&rec, SetStatus::Skipped)?;
    if let Some(id) = rec.id {
        store.mark_recommendation(id, "skipped")?;
    }
    store.exclude_exercise(&rec.movement_id)?;
    daemon::regenerate_queue_best_effort(env);
    Ok(())
}

fn exercises(env: &RuntimeEnv, command: ExerciseCommand) -> Result<()> {
    let store = Store::open(&env.paths.database_file)?;
    match command {
        ExerciseCommand::Removed => {
            let removed = store.removed_exercise_ids()?;
            if removed.is_empty() {
                println!("No exercises removed.");
            } else {
                for id in removed {
                    let name = crate::exercise_catalog::find(&id)
                        .map(|entry| entry.id.as_str())
                        .unwrap_or(&id);
                    println!("{name}");
                }
            }
        }
        ExerciseCommand::Restore { exercise_id } => {
            if store.restore_exercise(&exercise_id)? {
                println!("restored {exercise_id}");
            } else {
                bail!("exercise is not removed: {exercise_id}");
            }
        }
        ExerciseCommand::RestoreAll => {
            let count = store.removed_exercise_ids()?.len();
            store.clear_exercise_exclusions()?;
            println!("restored {count} exercise(s)");
        }
    }
    Ok(())
}

fn action_with_options(
    env: &RuntimeEnv,
    status: SetStatus,
    reps: Option<u32>,
    fatigued: bool,
    announce: bool,
) -> Result<()> {
    let paths = &env.paths;
    let store = Store::open(&paths.database_file)?;
    let Some(rec) = store.latest_open_recommendation()? else {
        bail!("no active recommendation");
    };
    let next_status = match status {
        SetStatus::Started => "active",
        SetStatus::Done => "done",
        SetStatus::Skipped => "skipped",
        SetStatus::Pain => "pain",
    };
    if status != SetStatus::Started {
        if let Some(reps) = reps {
            store.record_set_with_reps(&rec, status, reps)?;
        } else {
            store.record_set(&rec, status)?;
        }
        if fatigued {
            store.suppress_next_opportunities(5)?;
        }
    } else if let Some(id) = rec.id {
        store.set_state(AppStateKind::Active, Some(id), None, None)?;
    }
    if let Some(id) = rec.id {
        store.mark_recommendation(id, next_status)?;
    }
    if announce {
        println!("{next_status}");
    }
    Ok(())
}

fn text(value: impl std::fmt::Display) -> String {
    ansi(value, colors::TEXT, false)
}

fn text_bold(value: impl std::fmt::Display) -> String {
    ansi(value, colors::TEXT, true)
}

fn muted(value: impl std::fmt::Display) -> String {
    ansi(value, colors::MUTED, false)
}

fn ember(value: impl std::fmt::Display) -> String {
    ansi(value, colors::EMBER, false)
}

fn ember_bold(value: impl std::fmt::Display) -> String {
    ansi(value, colors::EMBER, true)
}

fn ansi(value: impl std::fmt::Display, color: (u8, u8, u8), bold: bool) -> String {
    let (r, g, b) = color;
    if bold {
        format!("\x1b[1;38;2;{r};{g};{b}m{value}\x1b[0m")
    } else {
        format!("\x1b[38;2;{r};{g};{b}m{value}\x1b[0m")
    }
}

mod colors {
    pub const TEXT: (u8, u8, u8) = (230, 230, 230);
    pub const MUTED: (u8, u8, u8) = (136, 136, 136);
    pub const EMBER: (u8, u8, u8) = (255, 140, 0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingProfileAction {
    Continue,
    DestroyProfile,
    DestroyAll,
}

fn confirm_full_reset() -> Result<()> {
    println!("This permanently removes your Svarog profile and all activity history.");
    print!("Type \"destroy all\" to reset all user data and restart onboarding: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_full_reset_confirmation(input.trim())
}

fn parse_full_reset_confirmation(value: &str) -> Result<()> {
    if value == "destroy all" {
        Ok(())
    } else {
        bail!("reset cancelled; type exactly \"destroy all\" to confirm")
    }
}

fn prompt_existing_profile_action() -> Result<ExistingProfileAction> {
    println!("{}", ember("Existing Svarog profile found."));
    print!(
        "{} {} {} {} {}: ",
        muted("Press Enter to continue, type"),
        ember("\"destroy profile\""),
        muted("to reset profile answers, or"),
        ember("\"destroy all\""),
        muted("to remove profile and history")
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_existing_profile_action(input.trim())
}

fn parse_existing_profile_action(value: &str) -> Result<ExistingProfileAction> {
    match value {
        "" => Ok(ExistingProfileAction::Continue),
        "destroy profile" => Ok(ExistingProfileAction::DestroyProfile),
        "destroy all" => Ok(ExistingProfileAction::DestroyAll),
        _ => bail!(
            "setup cancelled; press Enter to continue or type exactly \"destroy profile\" or \"destroy all\""
        ),
    }
}

fn prompt_string(label: &str, default: &str) -> Result<String> {
    print!("{} {}: ", text(label), muted(format!("[{default}]")));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn prompt_multiline_string(label: &str, default: &str) -> Result<String> {
    println!("{}", text(label));
    print!("{}: ", muted(format!("[{default}]")));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn prompt_parse<T>(label: &str, default: Option<T>) -> Result<Option<T>>
where
    T: std::str::FromStr + std::fmt::Display + Copy,
    T::Err: std::fmt::Display,
{
    let default_text = default.map(|value| value.to_string()).unwrap_or_default();
    print!("{} {}: ", text(label), muted(format!("[{default_text}]")));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        return Ok(default);
    }
    value
        .parse::<T>()
        .map(Some)
        .map_err(|err| anyhow::anyhow!("invalid {label}: {err}"))
}

fn prompt_parse_with_display<T>(
    label: &str,
    default: Option<T>,
    default_text: &str,
) -> Result<(Option<T>, bool)>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    print!("{} {}: ", text(label), muted(format!("[{default_text}]")));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        return Ok((default, false));
    }
    value
        .parse::<T>()
        .map(|parsed| (Some(parsed), true))
        .map_err(|err| anyhow::anyhow!("invalid {label}: {err}"))
}

fn prompt_imperial_height(current_cm: Option<u32>) -> Result<Option<u32>> {
    let default_text = current_cm.map(format_imperial_height).unwrap_or_default();
    let default = (!default_text.is_empty()).then(|| default_text.clone());
    let (height, changed) = prompt_parse_with_display(
        "Height ft/in (e.g. 5'11, 6 ft 1 in, 71 in)",
        default,
        &default_text,
    )?;
    if !changed {
        return Ok(current_cm);
    }
    height
        .as_deref()
        .map(parse_imperial_height_inches)
        .transpose()?
        .map(total_inches_to_cm)
        .transpose()
}

fn prompt_imperial_weight(current_kg: Option<f32>) -> Result<Option<f32>> {
    let default_lb = current_kg.map(kg_to_lb);
    let default_text = default_lb
        .map(|weight| format!("{weight:.1}"))
        .unwrap_or_default();
    let (weight_lb, changed) = prompt_parse_with_display("Weight lb", default_lb, &default_text)?;
    if !changed {
        return Ok(current_kg);
    }
    Ok(weight_lb.map(lb_to_kg))
}

fn cm_to_feet_inches(height_cm: u32) -> (u32, u32) {
    let total_inches = (f64::from(height_cm) / 2.54).round() as u32;
    (total_inches / 12, total_inches % 12)
}

pub(crate) fn format_imperial_height(height_cm: u32) -> String {
    let (feet, inches) = cm_to_feet_inches(height_cm);
    format!("{feet}'{inches}\"")
}

pub(crate) fn parse_imperial_height_inches(value: &str) -> Result<u32> {
    let compact = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let parsed = if let Some((feet, inches)) = compact.split_once('\'') {
        let inches = inches.strip_suffix('"').unwrap_or(inches);
        parse_feet_and_inches(feet, inches)
    } else if let Some(value) = compact.strip_suffix("in") {
        if let Some((feet, inches)) = value.split_once("ft") {
            parse_feet_and_inches(feet, inches)
        } else {
            value.parse::<u32>().ok()
        }
    } else {
        None
    };

    parsed.ok_or_else(|| {
        anyhow::anyhow!(
            "invalid Height ft/in: use a whole-number height such as 5'11, 6 ft 1 in, or 71 in"
        )
    })
}

fn parse_feet_and_inches(feet: &str, inches: &str) -> Option<u32> {
    let feet = feet.parse::<u32>().ok()?;
    let inches = inches.parse::<u32>().ok()?;
    if inches > 11 {
        return None;
    }
    feet.checked_mul(12)?.checked_add(inches)
}

fn total_inches_to_cm(total_inches: u32) -> Result<u32> {
    let height_cm = (f64::from(total_inches) * 2.54).round();
    if height_cm > f64::from(u32::MAX) {
        bail!("invalid height: value is too large");
    }
    Ok(height_cm as u32)
}

fn kg_to_lb(weight_kg: f32) -> f32 {
    weight_kg / 0.453_592_37
}

fn lb_to_kg(weight_lb: f32) -> f32 {
    weight_lb * 0.453_592_37
}

fn prompt_bool(label: &str, default: bool) -> Result<bool> {
    let default_text = if default { "Y/n" } else { "y/N" };
    print!("{} {}: ", text(label), muted(format!("[{default_text}]")));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.trim().is_empty() {
        return Ok(default);
    }
    Ok(matches!(
        value.to_lowercase().as_str(),
        "y" | "yes" | "true" | "1"
    ))
}

fn unit_system_from_metric(use_metric: bool) -> UnitSystem {
    if use_metric {
        UnitSystem::Metric
    } else {
        UnitSystem::Imperial
    }
}

fn prompt_list(label: &str, default: &[String]) -> Result<Vec<String>> {
    let default_text = default.join(", ");
    let value = prompt_string(label, &default_text)?;
    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Paths, RecommenderBackend, RuntimeMode};
    use clap::CommandFactory;
    use tempfile::tempdir;

    #[test]
    fn metric_prompt_choice_selects_unit_system() {
        assert_eq!(unit_system_from_metric(true), UnitSystem::Metric);
        assert_eq!(unit_system_from_metric(false), UnitSystem::Imperial);
    }

    #[test]
    fn imperial_height_converts_to_canonical_centimeters() {
        assert_eq!(total_inches_to_cm(71).unwrap(), 180);
        assert_eq!(total_inches_to_cm(77).unwrap(), 196);
        assert_eq!(cm_to_feet_inches(180), (5, 11));
        assert_eq!(cm_to_feet_inches(183), (6, 0));
        assert_eq!(format_imperial_height(180), "5'11\"");
    }

    #[test]
    fn imperial_height_parser_accepts_common_and_total_inches_forms() {
        for input in [
            "6'5",
            "6'5\"",
            "6 ft 5 in",
            "6FT5IN",
            " 6 ' 5 \" ",
            "77 in",
            "77IN",
        ] {
            assert_eq!(parse_imperial_height_inches(input).unwrap(), 77, "{input}");
        }
        assert_eq!(parse_imperial_height_inches("5'11").unwrap(), 71);
    }

    #[test]
    fn imperial_height_rejects_invalid_or_ambiguous_forms() {
        for input in [
            "",
            "77",
            "77\"",
            "6'",
            "'5",
            "6 ft in",
            "6 ft 12 in",
            "6.5 ft",
            "6'5 extra",
        ] {
            let error = parse_imperial_height_inches(input).unwrap_err();
            assert!(error.to_string().contains("5'11"), "{input}");
        }
    }

    #[test]
    fn pounds_convert_to_canonical_kilograms() {
        let kilograms = lb_to_kg(165.0);

        assert!((kilograms - 74.842_74).abs() < 0.000_1);
        assert!((kg_to_lb(kilograms) - 165.0).abs() < 0.000_1);
    }

    #[test]
    fn dry_run_setup_does_not_create_files() {
        let root = tempdir().unwrap().keep();
        let env = RuntimeEnv {
            mode: RuntimeMode::Dev,
            paths: Paths::from_root(root.join("svarog")),
            codex_home: root.join("codex"),
            daemon_addr: "127.0.0.1:18787".parse().unwrap(),
            dry_run: true,
        };

        setup_dry_run(&env).unwrap();

        assert!(!env.paths.config_file.exists());
        assert!(!env.paths.database_file.exists());
        assert!(!env.codex_home.join("hooks.json").exists());
    }

    #[test]
    fn cli_exposes_package_version() {
        let command = Cli::command();

        assert_eq!(command.get_version(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn cli_parses_explicit_setup_reset() {
        let production = Cli::try_parse_from(["svarog", "setup", "--reset"]).unwrap();
        assert!(matches!(
            production.command,
            Some(Command::Setup {
                dev: false,
                dry_run: false,
                reset: true
            })
        ));

        let dev = Cli::try_parse_from(["svarog", "setup", "--dev", "--reset"]).unwrap();
        assert!(matches!(
            dev.command,
            Some(Command::Setup {
                dev: true,
                dry_run: false,
                reset: true
            })
        ));

        assert!(Cli::try_parse_from(["svarog", "setup", "--reset", "--dry-run"]).is_err());
    }

    #[test]
    fn cli_parses_demo_commands() {
        let plain = Cli::try_parse_from(["svarog", "demo"]).unwrap();
        assert!(matches!(
            plain.command,
            Some(Command::Demo { remove_data: false })
        ));

        let reset = Cli::try_parse_from(["svarog", "demo", "--remove-data"]).unwrap();
        assert!(matches!(
            reset.command,
            Some(Command::Demo { remove_data: true })
        ));
    }

    #[test]
    fn cli_exposes_run_and_rejects_removed_tui_command() {
        let run = Cli::try_parse_from(["svarog", "run"]).unwrap();
        assert!(matches!(run.command, Some(Command::Run)));
        assert!(Cli::try_parse_from(["svarog", "tui"]).is_err());

        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("\n  run"));
        assert!(!help.contains("\n  tui"));
    }

    #[test]
    fn cli_exposes_stop_command() {
        let stop = Cli::try_parse_from(["svarog", "stop"]).unwrap();
        assert!(matches!(stop.command, Some(Command::Stop)));

        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("\n  stop"));
    }

    #[test]
    fn cli_parses_exercise_restore_commands() {
        let removed = Cli::try_parse_from(["svarog", "exercises", "removed"]).unwrap();
        assert!(matches!(
            removed.command,
            Some(Command::Exercises {
                command: ExerciseCommand::Removed
            })
        ));
        let restore = Cli::try_parse_from(["svarog", "exercises", "restore", "Dead_Bug"]).unwrap();
        assert!(matches!(
            restore.command,
            Some(Command::Exercises {
                command: ExerciseCommand::Restore { exercise_id }
            }) if exercise_id == "Dead_Bug"
        ));
    }

    #[test]
    fn cli_accepts_no_subcommand_for_default_launch() {
        let cli = Cli::try_parse_from(["svarog"]).unwrap();

        assert!(cli.command.is_none());
    }

    #[test]
    fn production_launch_only_needs_setup_without_a_config() {
        let root = tempdir().unwrap();
        let env = RuntimeEnv {
            mode: RuntimeMode::Production,
            paths: Paths::from_root(root.path().join("svarog")),
            codex_home: root.path().join("codex"),
            daemon_addr: "127.0.0.1:8787".parse().unwrap(),
            dry_run: false,
        };
        assert!(production_needs_setup(&env).unwrap());

        let mut config = Config::default();
        for step in config::CURRENT_ONBOARDING_STEPS {
            config.onboarding.mark_completed(step);
        }
        config::save(&env.paths, &config).unwrap();
        assert!(production_needs_setup(&env).unwrap());

        Store::open(&env.paths.database_file).unwrap();
        fs::create_dir_all(&env.codex_home).unwrap();
        fs::write(env.codex_home.join("hooks.json"), "{}").unwrap();

        assert!(!production_needs_setup(&env).unwrap());
    }

    #[test]
    fn onboarding_step_persists_answer_before_next_question() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();

        onboarding_step(&mut config, &paths, config::STEP_HEIGHT, false, |config| {
            config.profile.height_cm = Some(197);
            Ok(())
        })
        .unwrap();

        let resumed = config::load_or_default(&paths).unwrap();
        assert_eq!(resumed.profile.height_cm, Some(197));
        assert!(resumed.onboarding.is_completed(config::STEP_HEIGHT));
        assert!(!resumed.onboarding.is_completed(config::STEP_WEIGHT));
    }

    #[test]
    fn full_setup_reasks_a_completed_step() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.onboarding.mark_completed(config::STEP_HEIGHT);
        config.profile.height_cm = Some(180);

        onboarding_step(&mut config, &paths, config::STEP_HEIGHT, true, |config| {
            config.profile.height_cm = Some(197);
            Ok(())
        })
        .unwrap();

        assert_eq!(config.profile.height_cm, Some(197));
    }

    #[test]
    fn setup_repair_preserves_history_and_current_forge() {
        let root = tempdir().unwrap();
        let env = RuntimeEnv {
            mode: RuntimeMode::Production,
            paths: Paths::from_root(root.path().join("svarog")),
            codex_home: root.path().join("codex"),
            daemon_addr: "127.0.0.1:8787".parse().unwrap(),
            dry_run: false,
        };
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        for step in config::CURRENT_ONBOARDING_STEPS {
            config.onboarding.mark_completed(step);
        }
        let store = Store::open(&env.paths.database_file).unwrap();
        let event = crate::models::AgentEvent {
            agent: Agent::Codex,
            event: "user_prompt_submit".into(),
            expected_duration_sec: 60,
            project: Some("svarog".into()),
            created_at: chrono::Utc::now(),
        };
        store.insert_event(&event).unwrap();
        let current = crate::models::Recommendation {
            id: None,
            movement_id: "Dead_Bug".into(),
            movement_name: "Dead Bug".into(),
            primary_muscle: "abdominals".into(),
            muscles: vec!["abdominals".into()],
            reps: 4,
            weight_kg: None,
            estimated_seconds: 35,
            agent: Agent::Codex,
            project: Some("svarog".into()),
            side: None,
            created_at: chrono::Utc::now(),
        };
        let current_id = store.insert_recommendation(&current).unwrap();
        store.insert_queued_recommendation(&current).unwrap();
        store.exclude_exercise("Plank").unwrap();
        drop(store);

        finish_setup(&env, &config).unwrap();

        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(
            store.latest_open_recommendation().unwrap().unwrap().id,
            Some(current_id)
        );
        assert_eq!(
            store.queued_recommendation_count().unwrap(),
            recommender::QUEUE_TARGET
        );
        assert!(store.removed_exercise_ids().unwrap().is_empty());
        assert!(env.codex_home.join("hooks.json").exists());
    }

    #[test]
    fn explicit_reset_clears_history_and_restarts_onboarding() {
        let root = tempdir().unwrap();
        let env = RuntimeEnv {
            mode: RuntimeMode::Production,
            paths: Paths::from_root(root.path().join("svarog")),
            codex_home: root.path().join("codex"),
            daemon_addr: "127.0.0.1:8787".parse().unwrap(),
            dry_run: false,
        };
        let mut config = Config::default();
        config.profile.height_cm = Some(197);
        for step in config::CURRENT_ONBOARDING_STEPS {
            config.onboarding.mark_completed(step);
        }
        config::save(&env.paths, &config).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        store
            .insert_event(&crate::models::AgentEvent {
                agent: Agent::Codex,
                event: "user_prompt_submit".into(),
                expected_duration_sec: 60,
                project: Some("svarog".into()),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        store.exclude_exercise("Plank").unwrap();
        drop(store);

        reset_user_data(&env).unwrap();

        let reset_config = config::load_or_default(&env.paths).unwrap();
        assert_eq!(reset_config.profile.height_cm, None);
        assert_eq!(
            reset_config.onboarding.pending_steps(),
            config::CURRENT_ONBOARDING_STEPS
        );
        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 0);
        assert!(store.removed_exercise_ids().unwrap().is_empty());
        assert_eq!(store.state().unwrap().kind, AppStateKind::Idle);
    }

    #[test]
    fn demo_existing_data_defaults_to_resume() {
        assert_eq!(
            parse_demo_existing_action("").unwrap(),
            DemoExistingAction::Resume
        );
        assert_eq!(
            parse_demo_existing_action("remove").unwrap(),
            DemoExistingAction::Reset
        );
        assert!(parse_demo_existing_action("REMOVE").is_err());
        assert!(parse_demo_existing_action("remove demo data").is_err());
        assert!(parse_demo_existing_action("remove data").is_err());
    }

    #[test]
    fn demo_removal_does_not_touch_neighboring_production_data() {
        let project = tempdir().unwrap();
        let demo = project.path().join(".svarog-dev");
        let production = project.path().join("production");
        fs::create_dir_all(demo.join("svarog")).unwrap();
        fs::create_dir_all(&production).unwrap();
        fs::write(demo.join("svarog/config.toml"), "demo").unwrap();
        fs::write(production.join("history.sqlite3"), "keep").unwrap();

        remove_demo_data(&demo).unwrap();

        assert!(!demo.exists());
        assert_eq!(
            fs::read_to_string(production.join("history.sqlite3")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn demo_removal_rejects_any_other_directory_name() {
        let project = tempdir().unwrap();
        let invalid = project.path().join("production");
        fs::create_dir_all(&invalid).unwrap();

        assert!(remove_demo_data(&invalid).is_err());
        assert!(invalid.exists());
    }

    #[test]
    fn setup_style_helpers_emit_ansi_and_text() {
        let styled = ember("Svarog");
        let instruction = text_bold("Press Enter to accept defaults.");

        assert!(styled.contains("Svarog"));
        assert!(styled.contains("\u{1b}["));
        assert!(instruction.contains("Press Enter to accept defaults."));
        assert!(instruction.contains("\u{1b}[1;"));
    }

    #[test]
    fn existing_profile_action_defaults_to_continue() {
        assert_eq!(
            parse_existing_profile_action("").unwrap(),
            ExistingProfileAction::Continue
        );
    }

    #[test]
    fn parses_recommender_backend_names() {
        assert_eq!(
            "codex".parse::<RecommenderBackend>().unwrap(),
            RecommenderBackend::Codex
        );
        assert_eq!(
            "openai".parse::<RecommenderBackend>().unwrap(),
            RecommenderBackend::Openai
        );
        assert_eq!(
            "local".parse::<RecommenderBackend>().unwrap(),
            RecommenderBackend::Local
        );
        assert!("off".parse::<RecommenderBackend>().is_err());
        assert!("bad".parse::<RecommenderBackend>().is_err());
    }

    #[test]
    fn existing_profile_action_requires_exact_destroy_phrase() {
        assert_eq!(
            parse_existing_profile_action("destroy profile").unwrap(),
            ExistingProfileAction::DestroyProfile
        );
        assert_eq!(
            parse_existing_profile_action("destroy all").unwrap(),
            ExistingProfileAction::DestroyAll
        );
        assert!(parse_existing_profile_action("delete all").is_err());
        assert!(parse_existing_profile_action("DESTROY ALL").is_err());
    }

    #[test]
    fn full_reset_confirmation_requires_exact_destroy_phrase() {
        assert!(parse_full_reset_confirmation("destroy all").is_ok());
        assert!(parse_full_reset_confirmation("").is_err());
        assert!(parse_full_reset_confirmation("delete all").is_err());
        assert!(parse_full_reset_confirmation("DESTROY ALL").is_err());
    }
}
