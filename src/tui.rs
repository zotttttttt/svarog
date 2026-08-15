use crate::cli;
use crate::config::{
    self, Config, Forge, Paths, RecommenderBackend, RuntimeEnv, RuntimeMode, UnitSystem,
};
use crate::daemon::{self, ForgeNowResult, QueueRegenerationResult, QueueRegenerationStart};
use crate::exercise_catalog::{self, ExerciseCatalogEntry};
use crate::exercise_media::{self, PreparedGallery};
use crate::models::{
    AppStateKind, ForgeActivitySummary, Recommendation, RecommenderTokenProvider,
    RecommenderTokenUsageByProvider, SetStatus,
};
use crate::storage::{ForgeHistoryEntry, Store};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

type Spark = (usize, usize, char, bool);

const SPARK_BURSTS: [&[Spark]; 10] = [
    &[
        (0, 6, '˚', false),
        (0, 10, '⋆', true),
        (0, 14, '｡', false),
        (1, 8, '✧', true),
        (1, 10, '⋆', true),
        (1, 11, '✧', true),
        (1, 12, '˚', false),
        (2, 5, '｡', false),
        (2, 15, '˚', false),
    ],
    &[
        (0, 5, '｡', false),
        (0, 10, '⋆', true),
        (0, 12, '˚', false),
        (0, 15, '˚', false),
        (1, 7, '˚', false),
        (1, 9, '⋆', true),
        (1, 10, '˚', true),
        (1, 11, '✧', true),
        (1, 13, '˚', false),
        (2, 16, '｡', false),
    ],
    &[
        (0, 7, '˚', false),
        (0, 10, '⋆', true),
        (0, 13, '｡', false),
        (1, 6, '｡', false),
        (1, 9, '✧', true),
        (1, 10, '⋆', true),
        (1, 11, '✧', true),
        (1, 12, '˚', true),
        (1, 14, '｡', false),
        (2, 5, '˚', false),
    ],
    &[
        (0, 4, '｡', false),
        (0, 9, '˚', false),
        (0, 12, '✧', true),
        (0, 15, '˚', false),
        (1, 7, '˚', false),
        (1, 10, '⋆', true),
        (1, 11, '˚', true),
        (1, 12, '✧', true),
        (1, 14, '｡', false),
        (2, 15, '˚', false),
    ],
    &[
        (0, 6, '˚', false),
        (0, 9, '⋆', true),
        (0, 11, '｡', false),
        (0, 16, '｡', false),
        (1, 8, '✧', true),
        (1, 10, '⋆', true),
        (1, 11, '✧', true),
        (1, 13, '˚', false),
        (2, 5, '｡', false),
        (2, 15, '˚', false),
    ],
    &[
        (0, 5, '｡', false),
        (0, 8, '˚', false),
        (0, 11, '✧', true),
        (0, 14, '˚', false),
        (1, 7, '˚', false),
        (1, 9, '✧', true),
        (1, 10, '˚', true),
        (1, 11, '⋆', true),
        (1, 13, '｡', false),
        (2, 16, '｡', false),
    ],
    &[
        (0, 7, '｡', false),
        (0, 11, '⋆', true),
        (0, 15, '˚', false),
        (1, 6, '˚', false),
        (1, 9, '⋆', true),
        (1, 10, '✧', true),
        (1, 11, '✧', true),
        (1, 12, '˚', true),
        (1, 14, '｡', false),
        (2, 5, '｡', false),
    ],
    &[
        (0, 4, '˚', false),
        (0, 9, '⋆', true),
        (0, 12, '｡', false),
        (0, 16, '˚', false),
        (1, 8, '｡', false),
        (1, 10, '⋆', true),
        (1, 11, '✧', true),
        (1, 12, '˚', true),
        (1, 14, '˚', false),
        (2, 15, '｡', false),
    ],
    &[
        (0, 6, '｡', false),
        (0, 10, '˚', false),
        (0, 12, '✧', true),
        (0, 15, '｡', false),
        (1, 7, '˚', false),
        (1, 9, '✧', true),
        (1, 10, '˚', true),
        (1, 11, '⋆', true),
        (1, 13, '˚', false),
        (2, 5, '˚', false),
        (2, 16, '｡', false),
    ],
    &[
        (0, 5, '˚', false),
        (0, 11, '⋆', true),
        (0, 13, '｡', false),
        (0, 16, '˚', false),
        (1, 7, '｡', false),
        (1, 9, '⋆', true),
        (1, 10, '✧', true),
        (1, 11, '✧', true),
        (1, 12, '˚', true),
        (1, 14, '｡', false),
        (2, 15, '˚', false),
    ],
];

#[derive(Debug, Default)]
struct TuiState {
    recommendation_id: Option<i64>,
    actual_reps: u32,
    skip_check: bool,
    animation_frame: usize,
    status_message: Option<String>,
    show_help: bool,
    help_scroll: u16,
    exercise_media: Option<Receiver<Result<PreparedGallery, String>>>,
    exercise_media_id: Option<String>,
    exercise_media_feedback: Option<String>,
    show_history: bool,
    show_next: bool,
    queue_regeneration: Option<Receiver<QueueRegenerationResult>>,
    queue_regeneration_started_at: Option<Instant>,
    queue_regeneration_feedback: Option<QueueRegenerationFeedback>,
    queue_regeneration_feedback_started_at: Option<Instant>,
    forge_now_feedback: Option<String>,
    demo: bool,
    settings: Option<SettingsState>,
}

#[derive(Debug, Clone)]
struct SettingsState {
    draft: Config,
    row: usize,
    editing: bool,
    edit_value: String,
    selecting_archetype: bool,
    custom_archetype: bool,
    archetype_original: Option<Forge>,
    error: Option<String>,
}

const SETTINGS_ROWS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueueRegenerationFeedback {
    Success,
    Failure { no_safe_forges: bool },
}

#[derive(Debug)]
struct ViewModel {
    kind: ViewKind,
    recommendation: Option<Recommendation>,
    backend: BackendView,
    activity: ForgeActivitySummary,
    token_usage: RecommenderTokenUsageByProvider,
    history: Vec<ForgeHistoryEntry>,
    next_forges: Vec<Recommendation>,
}

#[derive(Debug, Clone)]
struct BackendView {
    label: String,
    unavailable: bool,
    config_file: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewKind {
    Idle,
    Forge,
    Cooldown,
}

pub fn run(env: &RuntimeEnv, shutdown: Arc<AtomicBool>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut ui = TuiState {
        demo: env.mode == RuntimeMode::Dev,
        ..TuiState::default()
    };
    let mut last_spark_toggle = Instant::now();
    let in_tmux = std::env::var_os("TMUX").is_some();

    let result: Result<()> = loop {
        if shutdown.load(Ordering::Acquire) {
            break Ok(());
        }
        if last_spark_toggle.elapsed() >= Duration::from_secs(1) {
            ui.animation_frame = (ui.animation_frame + 1) % (SPARK_BURSTS.len() * 2);
            last_spark_toggle = Instant::now();
        }

        poll_queue_regeneration(&mut ui);
        let view = load_view(&env.paths);
        if view.kind == ViewKind::Forge {
            ui.show_history = false;
            ui.show_next = false;
        }
        sync_reps(&mut ui, view.recommendation.as_ref());
        poll_exercise_media(&mut ui, view.recommendation.as_ref());
        terminal.draw(|frame| {
            let lines = if let Some(settings) = ui.settings.as_ref() {
                settings_lines(settings, ui.demo)
            } else {
                screen_lines(&view, &ui, in_tmux)
            };
            let mut paragraph = Paragraph::new(lines)
                .style(Style::default().bg(colors::BG).fg(colors::TEXT))
                .alignment(Alignment::Left);
            if ui.show_help && view.kind == ViewKind::Forge {
                paragraph = paragraph
                    .wrap(Wrap { trim: false })
                    .scroll((ui.help_scroll, 0));
            }
            frame.render_widget(paragraph, frame.area());
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if ui.settings.is_some() {
                    if let Err(error) = handle_settings_key(&mut ui, key.code, env) {
                        if let Some(settings) = ui.settings.as_mut() {
                            settings.error = Some(error.to_string());
                        }
                    }
                    continue;
                }
                if matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown)
                    && key.code == KeyCode::Char('s')
                {
                    if let Ok(draft) = config::load_or_default(&env.paths) {
                        ui.settings = Some(SettingsState {
                            draft,
                            row: 0,
                            editing: false,
                            edit_value: String::new(),
                            selecting_archetype: false,
                            custom_archetype: false,
                            archetype_original: None,
                            error: None,
                        });
                    }
                    continue;
                }
                if quit_requested(key) {
                    break Ok(());
                }
                if view.kind == ViewKind::Forge && ui.show_help {
                    match key.code {
                        KeyCode::Esc => {
                            ui.show_help = false;
                            ui.help_scroll = 0;
                        }
                        KeyCode::Up => ui.help_scroll = ui.help_scroll.saturating_sub(1),
                        KeyCode::Down => {
                            let area = terminal.size()?;
                            let limit =
                                help_scroll_limit(&view, &ui, in_tmux, area.width, area.height);
                            ui.help_scroll = ui.help_scroll.saturating_add(1).min(limit);
                        }
                        KeyCode::PageUp => {
                            ui.help_scroll = ui.help_scroll.saturating_sub(5);
                        }
                        KeyCode::PageDown => {
                            let area = terminal.size()?;
                            let limit =
                                help_scroll_limit(&view, &ui, in_tmux, area.width, area.height);
                            ui.help_scroll = ui.help_scroll.saturating_add(5).min(limit);
                        }
                        KeyCode::Home => ui.help_scroll = 0,
                        KeyCode::End => {
                            let area = terminal.size()?;
                            ui.help_scroll =
                                help_scroll_limit(&view, &ui, in_tmux, area.width, area.height);
                        }
                        KeyCode::Char('o') if ui.exercise_media.is_none() => {
                            if let Some(entry) = view
                                .recommendation
                                .as_ref()
                                .and_then(|rec| exercise_catalog::find(&rec.movement_id))
                            {
                                start_exercise_media(&mut ui, &env.paths, entry.clone());
                            } else {
                                ui.exercise_media_feedback =
                                    Some("No reference images are available.".into());
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if exercise_help_requested(key.code, view.kind) {
                    ui.show_help = true;
                    ui.help_scroll = 0;
                    ui.skip_check = false;
                    ui.exercise_media_feedback = None;
                    continue;
                }
                if regenerate_queue_requested(key.code, view.kind, ui.show_next) {
                    if ui.queue_regeneration.is_none() {
                        apply_queue_regeneration_start(&mut ui, daemon::regenerate_queue(env));
                    }
                    continue;
                }
                if forge_now_requested(key.code, view.kind, ui.show_history) {
                    ui.forge_now_feedback = None;
                    if ui.queue_regeneration.is_none() {
                        match daemon::forge_now(env) {
                            Ok(ForgeNowResult::Started) => {}
                            Ok(ForgeNowResult::NoQueued) => {
                                ui.forge_now_feedback =
                                    Some("No forges queued. Generating a fresh queue…".into());
                                apply_queue_regeneration_start(
                                    &mut ui,
                                    daemon::regenerate_queue(env),
                                );
                            }
                            Ok(ForgeNowResult::NoSafe) => {
                                ui.forge_now_feedback = Some(
                                    "No safe forges are available right now. Keeping current list."
                                        .into(),
                                );
                            }
                            Err(error) => {
                                ui.forge_now_feedback =
                                    Some(format!("Could not forge now: {error}"));
                            }
                        }
                    }
                    continue;
                }
                if let Some(panel) =
                    waiting_panel_for_key(key.code, view.kind, ui.show_history, ui.show_next)
                {
                    ui.show_history = panel == WaitingPanel::History;
                    ui.show_next = panel == WaitingPanel::Next;
                    continue;
                }
                match (key.code, view.kind, ui.skip_check) {
                    (code, ViewKind::Forge, false) if increase_reps_requested(code) => {
                        ui.actual_reps = ui.actual_reps.saturating_add(1).min(999);
                    }
                    (KeyCode::Char('-'), ViewKind::Forge, false) => {
                        ui.actual_reps = ui.actual_reps.saturating_sub(1).max(1);
                    }
                    (KeyCode::Char('d') | KeyCode::Enter, ViewKind::Forge, false) => {
                        let _ = cli::tui_action_with_reps(env, SetStatus::Done, ui.actual_reps);
                        ui.skip_check = false;
                    }
                    (KeyCode::Char('s'), ViewKind::Forge, false) => {
                        ui.skip_check = true;
                    }
                    (code, ViewKind::Forge, true) if skip_confirmation_action(code).is_some() => {
                        match skip_confirmation_action(code).unwrap() {
                            SkipConfirmationAction::Fatigued => {
                                let _ = cli::tui_action_skip_fatigued(env);
                            }
                            SkipConfirmationAction::Normal => {
                                let _ = cli::tui_action(env, SetStatus::Skipped);
                            }
                            SkipConfirmationAction::Remove => {
                                match cli::tui_action_remove_exercise(env) {
                                    Ok(()) => {
                                        ui.status_message = Some(
                                            "Exercise removed. Run `svarog exercises restore <id>` to restore it."
                                                .into(),
                                        );
                                    }
                                    Err(error) => {
                                        ui.status_message =
                                            Some(format!("Could not remove exercise: {error}"));
                                    }
                                }
                            }
                            SkipConfirmationAction::Cancel => {}
                        }
                        ui.skip_check = false;
                    }
                    (KeyCode::Char('p'), ViewKind::Forge, _) => {
                        let _ = cli::tui_action(env, SetStatus::Pain);
                        ui.skip_check = false;
                    }
                    _ => {}
                }
            }
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn apply_queue_regeneration_start(ui: &mut TuiState, start: QueueRegenerationStart) {
    match start {
        QueueRegenerationStart::Started(receiver) => {
            ui.queue_regeneration = Some(receiver);
            ui.queue_regeneration_started_at = Some(Instant::now());
            ui.queue_regeneration_feedback = None;
            ui.queue_regeneration_feedback_started_at = None;
            ui.forge_now_feedback = None;
        }
        QueueRegenerationStart::Busy => {}
    }
}

fn load_view(paths: &Paths) -> ViewModel {
    let backend = recommender_backend_view(paths);
    let Ok(store) = Store::open(&paths.database_file) else {
        return ViewModel {
            kind: ViewKind::Idle,
            recommendation: None,
            backend,
            activity: ForgeActivitySummary::default(),
            token_usage: RecommenderTokenUsageByProvider::default(),
            history: Vec::new(),
            next_forges: Vec::new(),
        };
    };
    let state = store.state().ok();
    let recommendation = store.latest_open_recommendation().ok().flatten();
    let activity = store.completed_forge_summary().unwrap_or_default();
    let token_usage = RecommenderTokenUsageByProvider {
        codex: store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap_or_default(),
        openai: store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::OpenAi)
            .unwrap_or_default(),
    };
    let history = store.recent_forge_history(10).unwrap_or_default();
    let next_forges = store.queued_recommendations().unwrap_or_default();
    let kind = match (
        state.as_ref().map(|state| state.kind),
        recommendation.as_ref(),
    ) {
        (Some(AppStateKind::Recommendation | AppStateKind::Active), Some(_)) => ViewKind::Forge,
        (Some(AppStateKind::Cooldown), _) => ViewKind::Cooldown,
        _ => ViewKind::Idle,
    };
    ViewModel {
        kind,
        recommendation,
        backend,
        activity,
        token_usage,
        history,
        next_forges,
    }
}

fn recommender_backend_view(paths: &Paths) -> BackendView {
    let config_file = paths.config_file.display().to_string();
    if !paths.config_file.exists() {
        return BackendView {
            label: "unknown".to_string(),
            unavailable: true,
            config_file,
        };
    }
    let Ok(config) = config::load_or_default(paths) else {
        return BackendView {
            label: "unknown".to_string(),
            unavailable: true,
            config_file,
        };
    };
    BackendView {
        label: config.recommender.backend.label().to_string(),
        unavailable: !backend_available(&config),
        config_file,
    }
}

fn backend_available(config: &config::Config) -> bool {
    match config.recommender.backend {
        RecommenderBackend::Codex => command_available(&config.recommender.codex.command),
        RecommenderBackend::Openai => std::env::var(&config.recommender.openai.api_key_env)
            .is_ok_and(|value| !value.trim().is_empty()),
        RecommenderBackend::Local => true,
    }
}

fn command_available(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    if command.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(command).is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|path| path.join(command).is_file())
}

fn sync_reps(ui: &mut TuiState, recommendation: Option<&Recommendation>) {
    let rec_id = recommendation.and_then(|rec| rec.id);
    if rec_id != ui.recommendation_id {
        ui.recommendation_id = rec_id;
        ui.actual_reps = recommendation.map(|rec| rec.reps).unwrap_or(0);
        ui.skip_check = false;
        ui.show_help = false;
        ui.help_scroll = 0;
        ui.exercise_media_feedback = None;
    }
}

fn start_exercise_media(ui: &mut TuiState, paths: &Paths, entry: ExerciseCatalogEntry) {
    let exercise_id = entry.id.clone();
    let data_dir = paths.data_dir.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = exercise_media::prepare_gallery(&data_dir, &entry)
            .map_err(|error| format!("Could not prepare images: {error:#}"));
        let _ = sender.send(result);
    });
    ui.exercise_media = Some(receiver);
    ui.exercise_media_id = Some(exercise_id);
    ui.exercise_media_feedback = None;
}

fn poll_exercise_media(ui: &mut TuiState, recommendation: Option<&Recommendation>) {
    let result = match ui.exercise_media.as_ref().map(Receiver::try_recv) {
        Some(Ok(result)) => Some(result),
        Some(Err(TryRecvError::Disconnected)) => Some(Err(
            "Could not prepare images: download worker stopped.".into(),
        )),
        Some(Err(TryRecvError::Empty)) | None => None,
    };
    let Some(result) = result else {
        return;
    };
    ui.exercise_media = None;
    let requested_id = ui.exercise_media_id.take();
    let active_id = recommendation.map(|rec| rec.movement_id.as_str());
    let still_viewing = ui.show_help && requested_id.as_deref() == active_id;
    if !still_viewing {
        return;
    }

    ui.exercise_media_feedback = Some(match result {
        Ok(gallery) => match exercise_media::open_gallery(&gallery.path) {
            Ok(()) => "Opened reference images in your browser.".into(),
            Err(error) => format!(
                "Images cached at {} but could not be opened: {error}",
                gallery.path.display()
            ),
        },
        Err(error) => error,
    });
}

fn poll_queue_regeneration(ui: &mut TuiState) {
    poll_queue_regeneration_at(ui, Instant::now());
}

fn poll_queue_regeneration_at(ui: &mut TuiState, now: Instant) {
    if matches!(
        ui.queue_regeneration_feedback,
        Some(QueueRegenerationFeedback::Success)
    ) && ui
        .queue_regeneration_feedback_started_at
        .is_some_and(|started_at| {
            now.saturating_duration_since(started_at) >= Duration::from_secs(3)
        })
    {
        ui.queue_regeneration_feedback = None;
        ui.queue_regeneration_feedback_started_at = None;
    }

    let result = match ui.queue_regeneration.as_ref().map(Receiver::try_recv) {
        Some(Ok(result)) => Some(result),
        Some(Err(TryRecvError::Disconnected)) => Some(Err(String::new())),
        Some(Err(TryRecvError::Empty)) | None => None,
    };
    let Some(result) = result else {
        return;
    };
    ui.queue_regeneration = None;
    ui.queue_regeneration_started_at = None;
    match result {
        Ok(_) => {
            ui.queue_regeneration_feedback = Some(QueueRegenerationFeedback::Success);
            ui.queue_regeneration_feedback_started_at = Some(now);
        }
        Err(error) => {
            ui.queue_regeneration_feedback = Some(QueueRegenerationFeedback::Failure {
                no_safe_forges: error.contains(crate::daemon::NO_SAFE_FORGES_ERROR),
            });
            ui.queue_regeneration_feedback_started_at = None;
        }
    }
}

fn queue_regeneration_loader(ui: &TuiState) -> Option<usize> {
    ui.queue_regeneration.as_ref()?;
    let elapsed = ui.queue_regeneration_started_at?.elapsed().as_millis();
    Some(queue_regeneration_loader_frame(elapsed))
}

fn queue_regeneration_loader_frame(elapsed_ms: u128) -> usize {
    (elapsed_ms / 200) as usize % 6
}

fn view_lines(view: &ViewModel, ui: &TuiState) -> Vec<Line<'static>> {
    if ui.show_history && matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown) {
        return history_lines(&view.history, ui.demo);
    }
    if ui.show_next && matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown) {
        return next_forge_lines(
            &view.next_forges,
            ui.demo,
            queue_regeneration_loader(ui),
            ui.queue_regeneration_feedback.as_ref(),
            ui.forge_now_feedback.as_deref(),
        );
    }
    if ui.show_help && view.kind == ViewKind::Forge {
        return view
            .recommendation
            .as_ref()
            .map(|rec| {
                exercise_help_lines(
                    rec,
                    exercise_catalog::find(&rec.movement_id),
                    ui.exercise_media.is_some()
                        && ui.exercise_media_id.as_deref() == Some(rec.movement_id.as_str()),
                    ui.exercise_media_feedback.as_deref(),
                    ui.demo,
                )
            })
            .unwrap_or_default();
    }
    match view.kind {
        ViewKind::Forge => view
            .recommendation
            .as_ref()
            .map(|rec| forge_lines(rec, ui))
            .unwrap_or_else(|| {
                idle_lines(
                    &view.backend,
                    &view.activity,
                    &view.token_usage,
                    ui.status_message.as_deref(),
                    queue_regeneration_loader(ui),
                    ui.queue_regeneration_feedback.as_ref(),
                    ui.forge_now_feedback.as_deref(),
                    ui.demo,
                )
            }),
        ViewKind::Cooldown => cooldown_lines(
            &view.backend,
            &view.activity,
            &view.token_usage,
            ui.status_message.as_deref(),
            queue_regeneration_loader(ui),
            ui.queue_regeneration_feedback.as_ref(),
            ui.forge_now_feedback.as_deref(),
            ui.demo,
        ),
        ViewKind::Idle => idle_lines(
            &view.backend,
            &view.activity,
            &view.token_usage,
            ui.status_message.as_deref(),
            queue_regeneration_loader(ui),
            ui.queue_regeneration_feedback.as_ref(),
            ui.forge_now_feedback.as_deref(),
            ui.demo,
        ),
    }
}

fn archetype_lines(forge: &Forge, custom_edit: Option<&str>, demo: bool) -> Vec<Line<'static>> {
    let archetype = crate::archetypes::get(forge.archetype);
    let title = if forge.archetype == crate::archetypes::ArchetypeId::Custom {
        forge.custom_archetype.as_deref().unwrap_or("Custom")
    } else {
        archetype.name
    };
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled(
            if archetype.symbol.chars().count() > 2 {
                archetype.fallback_symbol
            } else {
                archetype.symbol
            },
            accent_bold(),
        )),
        Line::from(Span::styled(title.to_uppercase(), text_bold())),
        Line::from(""),
        Line::from(Span::styled(archetype.description, text())),
        Line::from(""),
    ];
    for (label, score) in [
        ("Strength", archetype.stats.strength),
        ("Muscle", archetype.stats.muscle),
        ("Cardio", archetype.stats.cardio),
        ("Mobility", archetype.stats.mobility),
        ("Control", archetype.stats.control),
        ("Stamina", archetype.stats.stamina),
        ("Longevity", archetype.stats.longevity),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<10} "), muted()),
            Span::styled(
                format!(
                    "{}{}  {score}",
                    "█".repeat(score as usize),
                    "░".repeat(10 - score as usize)
                ),
                accent(),
            ),
        ]));
    }
    lines.push(Line::from(""));
    if let Some(value) = custom_edit {
        lines.push(Line::from(Span::styled("Custom archetype", text_bold())));
        lines.push(Line::from(Span::styled(format!("> {value}_"), accent())));
        lines.push(Line::from(Span::styled("[enter] Set  [esc] Back", muted())));
    } else {
        lines.push(Line::from(Span::styled(
            "←/h Previous   →/l Next   [enter] Choose   [/] Custom   [esc] Back",
            muted(),
        )));
        lines.push(Line::from(Span::styled(
            "You can change your archetype at any time.",
            muted(),
        )));
    }
    lines
}

fn settings_lines(settings: &SettingsState, demo: bool) -> Vec<Line<'static>> {
    if settings.selecting_archetype {
        return archetype_lines(
            &settings.draft.forge,
            settings
                .custom_archetype
                .then_some(settings.edit_value.as_str()),
            demo,
        );
    }
    let profile = &settings.draft.profile;
    let values = [
        (
            "Forge archetype",
            if settings.draft.forge.archetype == crate::archetypes::ArchetypeId::Custom {
                settings
                    .draft
                    .forge
                    .custom_archetype
                    .clone()
                    .unwrap_or_else(|| "Custom".into())
            } else {
                crate::archetypes::get(settings.draft.forge.archetype)
                    .name
                    .into()
            },
        ),
        (
            "Recommender",
            settings.draft.recommender.backend.label().into(),
        ),
        (
            "Notifications",
            if settings.draft.preferences.desktop_notifications {
                "enabled".into()
            } else {
                "disabled".into()
            },
        ),
        (
            "Daily safety ceiling",
            settings.draft.preferences.max_daily_sets.to_string(),
        ),
        ("Units", profile.unit_system.to_string()),
        (
            if profile.unit_system == UnitSystem::Metric {
                "Height (cm)"
            } else {
                "Height (in)"
            },
            profile
                .height_cm
                .map(|v| {
                    if profile.unit_system == UnitSystem::Metric {
                        v.to_string()
                    } else {
                        format!("{:.1}", v as f32 / 2.54)
                    }
                })
                .unwrap_or_else(|| "not set".into()),
        ),
        (
            if profile.unit_system == UnitSystem::Metric {
                "Weight (kg)"
            } else {
                "Weight (lb)"
            },
            profile
                .weight_kg
                .map(|v| {
                    if profile.unit_system == UnitSystem::Metric {
                        v.to_string()
                    } else {
                        format!("{:.1}", v * 2.204_622_6)
                    }
                })
                .unwrap_or_else(|| "not set".into()),
        ),
        (
            "Age",
            profile
                .age
                .map(|v| v.to_string())
                .unwrap_or_else(|| "not set".into()),
        ),
        ("Goals", profile.goals.join(", ")),
        ("Equipment", profile.equipment_text.clone()),
        ("Work position", profile.work_setup.clone()),
        (
            "Arm availability",
            if profile.one_hand_available {
                "alternate arms".into()
            } else {
                "both hands".into()
            },
        ),
        (
            "Cautious body parts",
            profile.cautious_body_parts.join(", "),
        ),
        ("Injuries / limitations", profile.injuries.join(", ")),
        ("Exercise preferences", profile.exercise_preferences.clone()),
        ("Apply changes", "Enter to save".into()),
    ];
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled("Settings", text_bold())),
        Line::from(Span::styled("↑/↓ Focus  ←/→ Change  Enter Edit", muted())),
        Line::from(""),
    ];
    for (index, (label, value)) in values.into_iter().enumerate() {
        let selected = index == settings.row;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                if selected { accent_bold() } else { muted() },
            ),
            Span::styled(
                format!("{label:<23}"),
                if selected { text_bold() } else { muted() },
            ),
            Span::styled(value, if selected { accent() } else { text() }),
        ]));
    }
    if settings.editing {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                format!("> {}_", settings.edit_value),
                accent(),
            )),
            Line::from(Span::styled(
                "[enter] Set field  [esc] Cancel edit",
                muted(),
            )),
        ]);
    } else {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("[esc] Cancel settings", muted())),
        ]);
    }
    if let Some(error) = settings.error.as_deref() {
        lines.push(Line::from(Span::styled(error.to_string(), accent())));
    }
    lines
}

fn begin_setting_edit(settings: &mut SettingsState) {
    settings.edit_value = match settings.row {
        5 => settings
            .draft
            .profile
            .height_cm
            .map(|v| {
                if settings.draft.profile.unit_system == UnitSystem::Metric {
                    v.to_string()
                } else {
                    format!("{:.1}", v as f32 / 2.54)
                }
            })
            .unwrap_or_default(),
        6 => settings
            .draft
            .profile
            .weight_kg
            .map(|v| {
                if settings.draft.profile.unit_system == UnitSystem::Metric {
                    v.to_string()
                } else {
                    format!("{:.1}", v * 2.204_622_6)
                }
            })
            .unwrap_or_default(),
        7 => settings
            .draft
            .profile
            .age
            .map(|v| v.to_string())
            .unwrap_or_default(),
        8 => settings.draft.profile.goals.join(", "),
        9 => settings.draft.profile.equipment_text.clone(),
        12 => settings.draft.profile.cautious_body_parts.join(", "),
        13 => settings.draft.profile.injuries.join(", "),
        14 => settings.draft.profile.exercise_preferences.clone(),
        _ => return,
    };
    settings.editing = true;
}

fn comma_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn commit_setting_edit(settings: &mut SettingsState) -> Result<()> {
    let value = settings.edit_value.trim();
    match settings.row {
        5 => {
            settings.draft.profile.height_cm = if value.is_empty() {
                None
            } else {
                let entered: f32 = value.parse().context("height must be a number")?;
                Some(
                    if settings.draft.profile.unit_system == UnitSystem::Metric {
                        entered
                    } else {
                        entered * 2.54
                    }
                    .round() as u32,
                )
            }
        }
        6 => {
            settings.draft.profile.weight_kg = if value.is_empty() {
                None
            } else {
                let entered: f32 = value.parse().context("weight must be a number")?;
                Some(
                    if settings.draft.profile.unit_system == UnitSystem::Metric {
                        entered
                    } else {
                        entered / 2.204_622_6
                    },
                )
            }
        }
        7 => {
            settings.draft.profile.age = if value.is_empty() {
                None
            } else {
                Some(value.parse().context("age must be a whole number")?)
            }
        }
        8 => settings.draft.profile.goals = comma_list(value),
        9 => {
            settings.draft.profile.equipment_text = if value.is_empty() {
                "bodyweight only".into()
            } else {
                value.into()
            }
        }
        12 => settings.draft.profile.cautious_body_parts = comma_list(value),
        13 => settings.draft.profile.injuries = comma_list(value),
        14 => {
            settings.draft.profile.exercise_preferences = if value.is_empty() {
                "automatic".into()
            } else {
                value.into()
            }
        }
        _ => {}
    }
    settings.editing = false;
    settings.error = None;
    Ok(())
}

fn apply_settings(env: &RuntimeEnv, draft: &Config) -> Result<()> {
    config::save(&env.paths, draft)?;
    let store = Store::open(&env.paths.database_file)?;
    store.save_user_profile(draft)?;
    let equipment = exercise_catalog::locally_resolved_equipment(&draft.profile.equipment_text);
    store.replace_movement_pool(&exercise_catalog::movements_for_equipment(&equipment))?;
    daemon::regenerate_queue_after_settings(env);
    Ok(())
}

fn handle_settings_key(ui: &mut TuiState, code: KeyCode, env: &RuntimeEnv) -> Result<()> {
    let Some(settings) = ui.settings.as_mut() else {
        return Ok(());
    };
    if settings.selecting_archetype {
        settings.error = None;
        if settings.custom_archetype {
            match code {
                KeyCode::Esc => {
                    settings.custom_archetype = false;
                    settings.edit_value.clear();
                }
                KeyCode::Enter if !settings.edit_value.trim().is_empty() => {
                    settings.draft.forge.archetype = crate::archetypes::ArchetypeId::Custom;
                    settings.draft.forge.custom_archetype =
                        Some(settings.edit_value.trim().chars().take(120).collect());
                    settings.custom_archetype = false;
                    settings.selecting_archetype = false;
                    settings.archetype_original = None;
                }
                KeyCode::Backspace => {
                    settings.edit_value.pop();
                }
                KeyCode::Char(ch) if settings.edit_value.chars().count() < 120 => {
                    settings.edit_value.push(ch)
                }
                _ => {}
            }
            return Ok(());
        }
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                settings.draft.forge.archetype =
                    crate::archetypes::next(settings.draft.forge.archetype, -1);
                settings.draft.forge.custom_archetype = None;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                settings.draft.forge.archetype =
                    crate::archetypes::next(settings.draft.forge.archetype, 1);
                settings.draft.forge.custom_archetype = None;
            }
            KeyCode::Char('/') => {
                settings.custom_archetype = true;
                settings.edit_value.clear();
            }
            KeyCode::Enter => {
                settings.selecting_archetype = false;
                settings.archetype_original = None;
            }
            KeyCode::Esc => {
                if let Some(original) = settings.archetype_original.take() {
                    settings.draft.forge = original;
                }
                settings.selecting_archetype = false;
            }
            _ => {}
        }
        return Ok(());
    }
    if settings.editing {
        match code {
            KeyCode::Esc => settings.editing = false,
            KeyCode::Enter => {
                commit_setting_edit(settings)?;
            }
            KeyCode::Backspace => {
                settings.edit_value.pop();
            }
            KeyCode::Char(ch) if settings.edit_value.chars().count() < 500 => {
                settings.edit_value.push(ch)
            }
            _ => {}
        }
        return Ok(());
    }
    match code {
        KeyCode::Esc => ui.settings = None,
        KeyCode::Up => settings.row = settings.row.saturating_sub(1),
        KeyCode::Down => settings.row = (settings.row + 1).min(SETTINGS_ROWS - 1),
        KeyCode::Left | KeyCode::Right => {
            let forward = code == KeyCode::Right;
            match settings.row {
                1 => {
                    settings.draft.recommender.backend = if forward {
                        settings.draft.recommender.backend.next()
                    } else {
                        settings.draft.recommender.backend.previous()
                    }
                }
                2 => {
                    settings.draft.preferences.desktop_notifications =
                        !settings.draft.preferences.desktop_notifications
                }
                3 => {
                    settings.draft.preferences.max_daily_sets = if forward {
                        settings
                            .draft
                            .preferences
                            .max_daily_sets
                            .saturating_add(1)
                            .min(1000)
                    } else {
                        settings
                            .draft
                            .preferences
                            .max_daily_sets
                            .saturating_sub(1)
                            .max(1)
                    }
                }
                4 => {
                    settings.draft.profile.unit_system =
                        if settings.draft.profile.unit_system == UnitSystem::Metric {
                            UnitSystem::Imperial
                        } else {
                            UnitSystem::Metric
                        }
                }
                10 => {
                    settings.draft.profile.work_setup =
                        if settings.draft.profile.work_setup == "sitting" {
                            "standing".into()
                        } else {
                            "sitting".into()
                        }
                }
                11 => {
                    settings.draft.profile.one_hand_available =
                        !settings.draft.profile.one_hand_available;
                    settings.draft.profile.two_hand_available =
                        !settings.draft.profile.one_hand_available;
                }
                _ => {}
            }
        }
        KeyCode::Enter if settings.row == 0 => {
            settings.archetype_original = Some(settings.draft.forge.clone());
            settings.selecting_archetype = true;
        }
        KeyCode::Enter if settings.row == SETTINGS_ROWS - 1 => {
            let draft = settings.draft.clone();
            apply_settings(env, &draft)?;
            ui.settings = None;
            ui.status_message = Some("Settings saved. Refreshing future forges…".into());
        }
        KeyCode::Enter => begin_setting_edit(settings),
        _ => {}
    }
    Ok(())
}

pub fn select_archetype(current: &Forge) -> Result<Option<Forge>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut forge = current.clone();
    let mut custom: Option<String> = None;
    let result = loop {
        terminal.draw(|frame| {
            let paragraph = Paragraph::new(archetype_lines(&forge, custom.as_deref(), false))
                .block(Block::default().borders(Borders::ALL).title(" SVAROG "))
                .style(Style::default().bg(colors::BG).fg(colors::TEXT))
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, frame.area());
        })?;
        if let Event::Key(key) = event::read()? {
            if let Some(value) = custom.as_mut() {
                match key.code {
                    KeyCode::Esc => custom = None,
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(ch) if value.chars().count() < 120 => value.push(ch),
                    KeyCode::Enter if !value.trim().is_empty() => {
                        forge.archetype = crate::archetypes::ArchetypeId::Custom;
                        forge.custom_archetype = Some(value.trim().to_string());
                        break Ok(Some(forge));
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Left | KeyCode::Char('h') => {
                        forge.archetype = crate::archetypes::next(forge.archetype, -1);
                        forge.custom_archetype = None;
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        forge.archetype = crate::archetypes::next(forge.archetype, 1);
                        forge.custom_archetype = None;
                    }
                    KeyCode::Char('/') => custom = Some(String::new()),
                    KeyCode::Enter => break Ok(Some(forge)),
                    KeyCode::Esc => break Ok(None),
                    _ => {}
                }
            }
        }
    };
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn screen_lines(view: &ViewModel, ui: &TuiState, in_tmux: bool) -> Vec<Line<'static>> {
    let mut lines = view_lines(view, ui);
    if in_tmux {
        lines.extend(tmux_control_lines());
    }
    lines.extend(quit_control_lines());
    lines
}

fn help_scroll_limit(
    view: &ViewModel,
    ui: &TuiState,
    in_tmux: bool,
    area_width: u16,
    area_height: u16,
) -> u16 {
    let width = usize::from(area_width.max(1));
    let rendered_height = screen_lines(view, ui, in_tmux)
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>();
    rendered_height
        .saturating_sub(usize::from(area_height))
        .min(usize::from(u16::MAX)) as u16
}

fn exercise_help_lines(
    rec: &Recommendation,
    entry: Option<&ExerciseCatalogEntry>,
    downloading: bool,
    feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled(rec.display_name(), accent_bold())),
        Line::from(Span::styled("How to", text_bold())),
        Line::from(""),
    ];
    match entry {
        Some(entry) if !entry.instructions.is_empty() => {
            for (index, instruction) in entry.instructions.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}. ", index + 1), accent_bold()),
                    Span::styled(instruction.clone(), text()),
                ]));
                lines.push(Line::from(""));
            }
        }
        _ => {
            lines.push(Line::from(Span::styled(
                "No written instructions are available for this exercise.",
                muted(),
            )));
            lines.push(Line::from(""));
        }
    }
    if downloading {
        lines.push(Line::from(Span::styled(
            "Downloading reference images…",
            accent(),
        )));
    } else if let Some(feedback) = feedback {
        lines.push(Line::from(Span::styled(feedback.to_string(), muted())));
    }
    lines.push(Line::from(Span::styled(
        "[o] Open images  [↑/↓] Scroll  [esc] Back",
        muted(),
    )));
    lines
}

#[allow(clippy::too_many_arguments)]
fn idle_lines(
    backend: &BackendView,
    activity: &ForgeActivitySummary,
    token_usage: &RecommenderTokenUsageByProvider,
    status_message: Option<&str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled("Waiting for the next forge.", muted())),
        Line::from(""),
        forge_now_control_line(),
        forge_list_controls_line(),
    ];
    lines.extend(waiting_forge_now_lines(
        queue_loader_frame,
        queue_feedback,
        forge_now_feedback,
    ));
    lines.push(Line::from(""));
    lines.push(recommender_line(backend));
    lines.extend(activity_lines(activity));
    lines.extend(recommender_usage_lines(backend, token_usage));
    if backend.unavailable {
        lines.push(Line::from(Span::styled(
            format!("Unavailable. Edit: {}", backend.config_file),
            muted(),
        )));
    }
    if let Some(message) = status_message {
        lines.push(Line::from(Span::styled(message.to_string(), muted())));
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn cooldown_lines(
    backend: &BackendView,
    activity: &ForgeActivitySummary,
    token_usage: &RecommenderTokenUsageByProvider,
    status_message: Option<&str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(vec![
            Span::styled("Forged. ", accent_bold()),
            Span::styled("Waiting for the next forge.", muted()),
        ]),
        Line::from(""),
        forge_now_control_line(),
        forge_list_controls_line(),
    ];
    lines.extend(waiting_forge_now_lines(
        queue_loader_frame,
        queue_feedback,
        forge_now_feedback,
    ));
    lines.push(Line::from(""));
    lines.push(recommender_line(backend));
    lines.extend(activity_lines(activity));
    lines.extend(recommender_usage_lines(backend, token_usage));
    if backend.unavailable {
        lines.push(Line::from(Span::styled(
            format!("Unavailable. Edit: {}", backend.config_file),
            muted(),
        )));
    }
    if let Some(message) = status_message {
        lines.push(Line::from(Span::styled(message.to_string(), muted())));
    }
    lines
}

fn activity_lines(activity: &ForgeActivitySummary) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled("Completed:", muted())),
        Line::from(vec![
            Span::styled("Today ", muted()),
            Span::styled(
                format!(
                    "{} forges / {} reps",
                    activity.today.forges, activity.today.reps
                ),
                text(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Week ", muted()),
            Span::styled(
                format!(
                    "{} forges / {} reps",
                    activity.week.forges, activity.week.reps
                ),
                text(),
            ),
        ]),
    ]
}

fn recommender_usage_lines(
    backend: &BackendView,
    usage: &RecommenderTokenUsageByProvider,
) -> Vec<Line<'static>> {
    let (title, usage, show_api_hint) = if backend.label == RecommenderBackend::Codex.label() {
        ("Svarog Codex tokens (in/out)", &usage.codex, true)
    } else if backend.label == RecommenderBackend::Openai.label() {
        ("Svarog OpenAI API tokens (in/out)", &usage.openai, false)
    } else {
        return Vec::new();
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(title, muted())),
        Line::from(vec![
            Span::styled("Today  ", muted()),
            Span::styled(
                format!(
                    "{} / {}",
                    compact_token_count(usage.today.input_tokens),
                    compact_token_count(usage.today.output_tokens)
                ),
                text(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Week   ", muted()),
            Span::styled(
                format!(
                    "{} / {}",
                    compact_token_count(usage.week.input_tokens),
                    compact_token_count(usage.week.output_tokens)
                ),
                text(),
            ),
        ]),
    ];
    if show_api_hint {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Use fewer Codex tokens with an OpenAI API key (separate billing):",
                muted(),
            )),
            Line::from(Span::styled("export OPENAI_API_KEY=\"...\"", muted())),
            Line::from(Span::styled(
                "Restart Svarog, then select [OpenAI API] with →.",
                muted(),
            )),
        ]);
    }
    lines
}

fn compact_token_count(tokens: u64) -> String {
    let (value, suffix) = if tokens >= 1_000_000 {
        (tokens as f64 / 1_000_000.0, "M")
    } else if tokens >= 1_000 {
        (tokens as f64 / 1_000.0, "k")
    } else {
        return tokens.to_string();
    };
    let formatted = format!("{value:.1}");
    format!("{}{suffix}", formatted.trim_end_matches(".0"))
}

fn recommender_line(backend: &BackendView) -> Line<'static> {
    Line::from(vec![
        Span::styled("Recommender: ", muted()),
        Span::styled(format!("[{}]", backend.label), text()),
        Span::styled("  [s] Settings", muted()),
    ])
}

fn tmux_control_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled("Click pane to focus.", muted())),
        Line::from(Span::styled("Drag border to resize.", muted())),
        Line::from(Span::styled("Ctrl-b + ←/→ switches panes.", muted())),
    ]
}

fn quit_control_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled("[q] Quit", muted())),
    ]
}

fn quit_requested(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE
}

fn exercise_help_requested(code: KeyCode, kind: ViewKind) -> bool {
    kind == ViewKind::Forge && matches!(code, KeyCode::Char('i') | KeyCode::Char('?'))
}

fn increase_reps_requested(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char('+') | KeyCode::Char('='))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitingPanel {
    Main,
    History,
    Next,
}

fn waiting_panel_for_key(
    code: KeyCode,
    kind: ViewKind,
    history_visible: bool,
    next_visible: bool,
) -> Option<WaitingPanel> {
    if !matches!(kind, ViewKind::Idle | ViewKind::Cooldown) {
        return None;
    }
    match code {
        KeyCode::Char('l') => Some(WaitingPanel::History),
        KeyCode::Char('n') => Some(WaitingPanel::Next),
        KeyCode::Esc if history_visible || next_visible => Some(WaitingPanel::Main),
        _ => None,
    }
}

fn forge_now_requested(code: KeyCode, kind: ViewKind, history_visible: bool) -> bool {
    code == KeyCode::Char('f')
        && !history_visible
        && matches!(kind, ViewKind::Idle | ViewKind::Cooldown)
}

fn regenerate_queue_requested(code: KeyCode, kind: ViewKind, next_visible: bool) -> bool {
    code == KeyCode::Char('r')
        && next_visible
        && matches!(kind, ViewKind::Idle | ViewKind::Cooldown)
}

fn forge_list_controls_line() -> Line<'static> {
    Line::from(Span::styled("[l] Latest forges [n] Next forges", muted()))
}

fn forge_now_control_line() -> Line<'static> {
    Line::from(Span::styled("[f] Forge now", muted()))
}

fn waiting_forge_now_lines(
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(frame) = queue_loader_frame {
        lines.push(queue_regeneration_loader_line(frame));
        return lines;
    }
    if let Some(message) = forge_now_feedback {
        lines.push(Line::from(Span::styled(message.to_string(), muted())));
        return lines;
    }
    match queue_feedback {
        Some(QueueRegenerationFeedback::Success) => {
            lines.push(Line::from(Span::styled("✓ Forges generated", accent())))
        }
        Some(QueueRegenerationFeedback::Failure { no_safe_forges }) => {
            let message = if *no_safe_forges {
                "No safe forges are available right now."
            } else {
                "Could not generate forges."
            };
            lines.push(Line::from(Span::styled(message, muted())));
        }
        None => {}
    }
    lines
}

fn queue_regeneration_loader_line(filled_segments: usize) -> Line<'static> {
    let mut spans = Vec::with_capacity(9);
    for segment in 0..5 {
        if segment > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if segment < filled_segments {
            accent()
        } else {
            muted()
        };
        spans.push(Span::styled("━━━━", style));
    }
    Line::from(spans)
}

fn next_forge_lines(
    next_forges: &[Recommendation],
    demo: bool,
    regeneration_loader_frame: Option<usize>,
    feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled("Next forges", text_bold())),
    ];
    if next_forges.is_empty() {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("No forges queued yet.", muted())),
        ]);
    } else {
        lines.push(Line::from(""));
        for (index, recommendation) in next_forges.iter().enumerate() {
            let mut label = format!(
                "{}. {} {}",
                index + 1,
                recommendation.reps,
                recommendation.display_name()
            );
            if let Some(weight) = recommendation.weight_kg {
                label.push_str(&format!(" · {}", weight_label(weight)));
            }
            lines.push(Line::from(Span::styled(label, text())));
        }
    }
    lines.push(Line::from(""));
    if let Some(frame) = regeneration_loader_frame {
        lines.push(queue_regeneration_loader_line(frame));
    } else {
        if let Some(message) = forge_now_feedback {
            lines.push(Line::from(Span::styled(message.to_string(), muted())));
        }
        match feedback {
            Some(QueueRegenerationFeedback::Success) => {
                lines.push(Line::from(Span::styled("✓ Forges generated", accent())))
            }
            Some(QueueRegenerationFeedback::Failure { no_safe_forges }) => {
                let message = if *no_safe_forges {
                    "No safe forges are available right now. Keeping current list."
                } else {
                    "Could not regenerate forges. Keeping current list."
                };
                lines.push(Line::from(Span::styled(message, muted())));
            }
            None => {}
        }
        lines.push(Line::from(Span::styled(
            "[f] Forge now  [r] Regenerate forges",
            muted(),
        )));
    }
    lines.push(Line::from(Span::styled("[esc] Back", muted())));
    lines
}

fn history_lines(history: &[ForgeHistoryEntry], demo: bool) -> Vec<Line<'static>> {
    history_lines_for_date(history, demo, Local::now().date_naive())
}

fn history_lines_for_date(
    history: &[ForgeHistoryEntry],
    demo: bool,
    today: chrono::NaiveDate,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        title_line(demo),
        Line::from(""),
        Line::from(Span::styled("Latest forges", text_bold())),
    ];
    if history.is_empty() {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("No forges yet.", muted())),
        ]);
    } else {
        let mut previous_date = None;
        for entry in history {
            let date = entry.created_at.with_timezone(&Local).date_naive();
            if previous_date != Some(date) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    history_date_label(date, today),
                    muted(),
                )));
                previous_date = Some(date);
            }
            lines.push(history_entry_line(entry));
        }
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled("[esc] Back", muted())),
    ]);
    lines
}

fn history_date_label(date: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    if date == today {
        "Today".to_string()
    } else if date == today - ChronoDuration::days(1) {
        "Yesterday".to_string()
    } else if date.year() == today.year() {
        format!("{} {}", date.format("%b"), date.day())
    } else {
        format!("{} {}, {}", date.format("%b"), date.day(), date.year())
    }
}

fn history_entry_line(entry: &ForgeHistoryEntry) -> Line<'static> {
    match entry.status.as_str() {
        "done" => {
            let mut label = format!("{} {}", entry.reps, entry.movement_name);
            if let Some(weight) = entry.weight_kg {
                label.push_str(&format!(" · {}", weight_label(weight)));
            }
            Line::from(vec![
                Span::styled("✓ ", accent_bold()),
                Span::styled(label, text()),
            ])
        }
        "skipped" => Line::from(vec![
            Span::styled("– ", muted()),
            Span::styled(format!("Skipped · {}", entry.movement_name), text()),
        ]),
        "pain" => Line::from(vec![
            Span::styled("! ", accent_bold()),
            Span::styled(format!("Pain · {}", entry.movement_name), text()),
        ]),
        status => Line::from(vec![
            Span::styled("? ", muted()),
            Span::styled(format!("{status} · {}", entry.movement_name), text()),
        ]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipConfirmationAction {
    Fatigued,
    Normal,
    Remove,
    Cancel,
}

fn skip_confirmation_action(code: KeyCode) -> Option<SkipConfirmationAction> {
    match code {
        KeyCode::Char('y') => Some(SkipConfirmationAction::Fatigued),
        KeyCode::Char('n') => Some(SkipConfirmationAction::Normal),
        KeyCode::Backspace => Some(SkipConfirmationAction::Remove),
        KeyCode::Esc => Some(SkipConfirmationAction::Cancel),
        _ => None,
    }
}

fn forge_lines(rec: &Recommendation, ui: &TuiState) -> Vec<Line<'static>> {
    if ui.skip_check {
        return vec![
            title_line(ui.demo),
            Line::from(""),
            Line::from(Span::styled("Skip this forge?", accent_bold())),
            Line::from(""),
            Line::from(Span::styled("Are you fatigued?", text())),
            Line::from(Span::styled(
                "This skips the next 5 opportunities.",
                muted(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("[y] Yes  ", accent_bold()),
                Span::styled("[n] No", muted()),
            ]),
            Line::from(Span::styled(
                "[backspace] Skip and remove this exercise",
                muted(),
            )),
            Line::from(Span::styled("[esc] Cancel", muted())),
        ];
    }

    let mut lines = vec![
        title_line(ui.demo),
        Line::from(""),
        Line::from(Span::styled(
            rec.display_name().to_uppercase(),
            accent_bold(),
        )),
    ];
    if let Some(weight) = rec.weight_kg {
        lines.push(Line::from(Span::styled(weight_label(weight), text())));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled("Target", muted())),
        Line::from(Span::styled(format!("{} reps", rec.reps), text_bold())),
        Line::from(""),
    ]);
    lines.extend(animation_lines(ui.animation_frame));
    lines.extend([
        Line::from(""),
        Line::from(vec![
            Span::styled("[d] Done  ", muted()),
            Span::styled("[s] Skip", muted()),
        ]),
        Line::from(vec![
            Span::styled("Actual reps: ", muted()),
            Span::styled(format!("{}", ui.actual_reps.max(1)), accent_bold()),
            Span::styled("  [+/-]", muted()),
        ]),
        Line::from(Span::styled("[i] How to", muted())),
    ]);
    lines
}

fn weight_label(weight: f32) -> String {
    if weight.fract() == 0.0 {
        format!("{} kg", weight as u32)
    } else {
        format!("{weight:.1} kg")
    }
}

fn animation_lines(frame: usize) -> Vec<Line<'static>> {
    let sparks = (frame % 2 == 1).then(|| SPARK_BURSTS[(frame / 2) % SPARK_BURSTS.len()]);
    let mut rows = vec![vec![(' ', muted()); 21]; 5];
    let active_style = if sparks.is_some() {
        accent_bold()
    } else {
        muted()
    };

    for (offset, character) in "___┬___".chars().enumerate() {
        rows[2][7 + offset] = (
            character,
            if character == '┬' {
                active_style
            } else {
                muted()
            },
        );
    }
    rows[3][10] = ('▔', muted());

    for (column, character) in "[FORGING IN PROGRESS]".chars().enumerate() {
        rows[4][column] = (character, active_style);
    }

    if let Some(sparks) = sparks {
        for &(row, column, character, amber) in sparks {
            rows[row][column] = (character, if amber { accent_bold() } else { muted() });
        }
    }

    rows.into_iter()
        .map(|mut cells| {
            while cells.last().is_some_and(|(character, _)| *character == ' ') {
                cells.pop();
            }
            Line::from(
                cells
                    .into_iter()
                    .map(|(character, style)| Span::styled(character.to_string(), style))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn title_line(demo: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled("⚒ ", accent_bold()),
        Span::styled("Svarog", text_bold()),
    ];
    if demo {
        spans.push(Span::styled("  [demo]", muted()));
    }
    Line::from(spans)
}

fn text() -> Style {
    Style::default().fg(colors::TEXT)
}

fn text_bold() -> Style {
    text().add_modifier(Modifier::BOLD)
}

fn muted() -> Style {
    Style::default().fg(colors::MUTED)
}

fn accent() -> Style {
    Style::default().fg(colors::EMBER)
}

fn accent_bold() -> Style {
    accent().add_modifier(Modifier::BOLD)
}

mod colors {
    use ratatui::style::Color;

    pub const BG: Color = Color::Rgb(7, 8, 8);
    pub const TEXT: Color = Color::Rgb(230, 230, 230);
    pub const MUTED: Color = Color::Rgb(136, 136, 136);
    pub const EMBER: Color = Color::Rgb(255, 140, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Recommender, RecommenderBackend};
    use crate::models::{
        Agent, ForgeActivityTotals, RecommenderTokenUsageSummary, TokenUsageTotals,
    };
    use crate::recommender::QueueGenerationSource;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn settings_state() -> SettingsState {
        SettingsState {
            draft: Config::default(),
            row: 0,
            editing: false,
            edit_value: String::new(),
            selecting_archetype: false,
            custom_archetype: false,
            archetype_original: None,
            error: None,
        }
    }

    #[test]
    fn settings_show_focus_and_archetype_opens_full_selector() {
        let mut settings = settings_state();
        let text = settings_lines(&settings, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("› Forge archetype"));
        assert!(text.contains("Apply changes"));

        settings.selecting_archetype = true;
        let selector = settings_lines(&settings, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(selector.contains("ATHLETE"));
        assert!(selector.contains("Strength"));
        assert!(selector.contains("You can change your archetype at any time."));
    }

    #[test]
    fn settings_text_edits_are_staged_until_apply() {
        let mut settings = settings_state();
        settings.row = 8;
        begin_setting_edit(&mut settings);
        settings.edit_value = "mobility, strength".into();
        commit_setting_edit(&mut settings).unwrap();
        assert_eq!(settings.draft.profile.goals, vec!["mobility", "strength"]);
    }

    fn rec() -> Recommendation {
        Recommendation {
            id: Some(1),
            movement_id: "left_curl".into(),
            movement_name: "left curl".into(),
            primary_muscle: "biceps".into(),
            muscles: vec!["biceps".into()],
            reps: 10,
            weight_kg: Some(12.0),
            estimated_seconds: 60,
            agent: Agent::Codex,
            project: None,
            side: None,
            created_at: Utc::now(),
        }
    }

    fn history_entry(
        movement_name: &str,
        status: &str,
        reps: u32,
        weight_kg: Option<f32>,
        created_at: chrono::DateTime<Utc>,
    ) -> ForgeHistoryEntry {
        ForgeHistoryEntry {
            movement_name: movement_name.to_string(),
            status: status.to_string(),
            reps,
            weight_kg,
            created_at,
        }
    }

    #[test]
    fn idle_lines_are_minimal() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/config.toml".to_string(),
        };
        let text = idle_lines(
            &backend,
            &ForgeActivitySummary::default(),
            &RecommenderTokenUsageByProvider::default(),
            None,
            None,
            None,
            None,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("Svarog"));
        assert!(text.contains("Waiting for the next forge."));
        assert!(text.contains("[l] Latest forges"));
        assert!(text.contains("[n] Next forges"));
        assert!(text.contains(
            "Waiting for the next forge.\n\n[f] Forge now\n[l] Latest forges [n] Next forges\n\nRecommender: [Codex]  [s] Settings"
        ));
        assert!(text.contains("Recommender: [Codex]  [s] Settings"));
        assert!(!text.contains("[r] Change recommender"));
        assert!(text.contains("Completed:"));
        assert!(text.contains("Svarog Codex tokens (in/out)"));
        assert!(text.contains("Today 0 forges / 0 reps"));
        assert!(text.contains("Week 0 forges / 0 reps"));
        assert!(text.contains("Use fewer Codex tokens with an OpenAI API key"));
        assert!(text.contains("export OPENAI_API_KEY=\"...\""));
        assert!(text.contains("Restart Svarog, then select [OpenAI API] with →."));
        assert!(!text.contains("sets"));
    }

    #[test]
    fn idle_and_cooldown_lines_show_compact_codex_usage() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/config.toml".to_string(),
        };
        let usage = RecommenderTokenUsageByProvider {
            codex: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 12_400,
                    output_tokens: 320,
                },
                week: TokenUsageTotals {
                    input_tokens: 58_100,
                    output_tokens: 1_400,
                },
            },
            openai: RecommenderTokenUsageSummary::default(),
        };
        let activity = ForgeActivitySummary {
            today: ForgeActivityTotals {
                forges: 3,
                reps: 42,
            },
            week: ForgeActivityTotals {
                forges: 12,
                reps: 180,
            },
        };

        for lines in [
            idle_lines(&backend, &activity, &usage, None, None, None, None, false),
            cooldown_lines(&backend, &activity, &usage, None, None, None, None, false),
        ] {
            let text = lines
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("Completed:"));
            assert!(text.contains("[l] Latest forges"));
            assert!(text.contains("[n] Next forges"));
            assert!(text.contains("Today 3 forges / 42 reps"));
            assert!(text.contains("Week 12 forges / 180 reps"));
            assert!(text.contains("Svarog Codex tokens (in/out)"));
            assert!(text.contains("Today  12.4k / 320"));
            assert!(text.contains("Week   58.1k / 1.4k"));
            assert!(text.contains("Use fewer Codex tokens with an OpenAI API key"));
            assert!(
                text.find("Completed:").unwrap()
                    < text.find("Svarog Codex tokens (in/out)").unwrap()
            );
        }
    }

    #[test]
    fn cooldown_status_combines_amber_forged_and_muted_waiting_text() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/config.toml".to_string(),
        };
        let lines = cooldown_lines(
            &backend,
            &ForgeActivitySummary::default(),
            &RecommenderTokenUsageByProvider::default(),
            None,
            None,
            None,
            None,
            false,
        );
        let status = lines
            .iter()
            .find(|line| line.to_string() == "Forged. Waiting for the next forge.")
            .expect("combined cooldown status line");

        assert_eq!(status.spans[0].style.fg, Some(colors::EMBER));
        assert_eq!(status.spans[1].style.fg, Some(colors::MUTED));
        assert!(lines.windows(5).any(|window| {
            window[0].to_string() == "Forged. Waiting for the next forge."
                && window[1].to_string().is_empty()
                && window[2].to_string() == "[f] Forge now"
                && window[3].to_string() == "[l] Latest forges [n] Next forges"
                && window[4].to_string().is_empty()
        }));
    }

    #[test]
    fn active_forge_omits_codex_usage_summary() {
        let view = ViewModel {
            kind: ViewKind::Forge,
            recommendation: Some(rec()),
            backend: BackendView {
                label: "Codex".to_string(),
                unavailable: false,
                config_file: "/tmp/config.toml".to_string(),
            },
            activity: ForgeActivitySummary {
                today: ForgeActivityTotals {
                    forges: 3,
                    reps: 42,
                },
                week: ForgeActivityTotals::default(),
            },
            token_usage: RecommenderTokenUsageByProvider {
                codex: RecommenderTokenUsageSummary {
                    today: TokenUsageTotals {
                        input_tokens: 12_400,
                        output_tokens: 320,
                    },
                    week: TokenUsageTotals::default(),
                },
                openai: RecommenderTokenUsageSummary::default(),
            },
            history: Vec::new(),
            next_forges: Vec::new(),
        };
        let text = view_lines(
            &view,
            &TuiState {
                show_history: true,
                ..TuiState::default()
            },
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(!text.contains("Svarog Codex tokens"));
        assert!(!text.contains("Completed:"));
        assert!(!text.contains("[l] Latest forges"));
        assert!(!text.contains("[n] Next forges"));
        assert!(text.contains("LEFT CURL"));
    }

    #[test]
    fn history_lines_group_dates_and_format_outcomes() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let at_noon = |date: chrono::NaiveDate| {
            Local
                .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
                .single()
                .unwrap()
                .with_timezone(&Utc)
        };
        let history = vec![
            history_entry("scapular squeezes", "done", 8, Some(12.0), at_noon(today)),
            history_entry(
                "left curls",
                "skipped",
                10,
                Some(12.0),
                at_noon(today - ChronoDuration::days(1)),
            ),
            history_entry(
                "desk posture reset",
                "pain",
                4,
                None,
                at_noon(chrono::NaiveDate::from_ymd_opt(2025, 12, 30).unwrap()),
            ),
        ];

        let text = history_lines_for_date(&history, false, today)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Today\n✓ 8 scapular squeezes · 12 kg"));
        assert!(text.contains("Yesterday\n– Skipped · left curls"));
        assert!(!text.contains("Skipped · left curls · 12 kg"));
        assert!(text.contains("Dec 30, 2025\n! Pain · desk posture reset"));
        assert!(text.contains("[esc] Back"));
    }

    #[test]
    fn history_lines_show_empty_state() {
        let text = history_lines_for_date(
            &[],
            false,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("Latest forges"));
        assert!(text.contains("No forges yet."));
        assert!(text.contains("[esc] Back"));
    }

    #[test]
    fn forge_list_navigation_is_only_available_while_waiting() {
        assert_eq!(
            waiting_panel_for_key(KeyCode::Char('l'), ViewKind::Idle, false, false),
            Some(WaitingPanel::History)
        );
        assert_eq!(
            waiting_panel_for_key(KeyCode::Char('n'), ViewKind::Cooldown, false, false),
            Some(WaitingPanel::Next)
        );
        assert_eq!(
            waiting_panel_for_key(KeyCode::Esc, ViewKind::Idle, true, false),
            Some(WaitingPanel::Main)
        );
        assert_eq!(
            waiting_panel_for_key(KeyCode::Esc, ViewKind::Idle, false, false),
            None
        );
        assert_eq!(
            waiting_panel_for_key(KeyCode::Char('n'), ViewKind::Forge, false, false),
            None
        );
        assert_eq!(
            waiting_panel_for_key(KeyCode::Char('f'), ViewKind::Idle, false, true),
            None
        );
        assert!(forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Idle,
            false
        ));
        assert!(forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Cooldown,
            false
        ));
        assert!(!forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Idle,
            true
        ));
        assert!(!forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Forge,
            false
        ));
    }

    #[test]
    fn next_forge_lines_show_queue_order_and_weight() {
        let mut first = rec();
        first.movement_name = "scapular squeezes".to_string();
        first.reps = 8;
        first.weight_kg = None;
        let mut second = rec();
        second.movement_name = "left curls".to_string();
        second.reps = 10;
        second.weight_kg = Some(12.0);

        let text = next_forge_lines(&[first, second], false, None, None, None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Next forges"));
        assert!(text.contains("1. 8 scapular squeezes"));
        assert!(text.contains("2. 10 left curls · 12 kg"));
        assert!(text.contains("[f] Forge now  [r] Regenerate forges"));
        assert!(text.contains("[esc] Back"));
    }

    #[test]
    fn next_forge_lines_show_empty_queue() {
        let text = next_forge_lines(&[], false, None, None, None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("No forges queued yet."));
    }

    #[test]
    fn next_forge_lines_show_regeneration_states() {
        let loading = next_forge_lines(&[rec()], false, Some(3), None, None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(loading.contains("━━━━ ━━━━ ━━━━ ━━━━ ━━━━"));
        assert!(!loading.contains("[r] Regenerate forges"));

        let success_feedback = QueueRegenerationFeedback::Success;
        let success = next_forge_lines(&[], false, None, Some(&success_feedback), None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(success.contains("✓ Forges generated"));
        assert!(!success.contains("local fallback"));
        assert!(success.contains("[r] Regenerate forges"));

        let failure_feedback = QueueRegenerationFeedback::Failure {
            no_safe_forges: false,
        };
        let failure = next_forge_lines(&[], false, None, Some(&failure_feedback), None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(failure.contains("Could not regenerate forges. Keeping current list."));
        assert!(failure.contains("[r] Regenerate forges"));

        let no_safe_feedback = QueueRegenerationFeedback::Failure {
            no_safe_forges: true,
        };
        let no_safe = next_forge_lines(&[], false, None, Some(&no_safe_feedback), None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(no_safe.contains("No safe forges are available right now. Keeping current list."));
    }

    #[test]
    fn next_forge_lines_show_manual_forge_feedback() {
        let text = next_forge_lines(
            &[rec()],
            false,
            None,
            None,
            Some("No safe forges are available right now. Keeping current list."),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("No safe forges are available right now. Keeping current list."));
        assert!(text.contains("[f] Forge now  [r] Regenerate forges"));
    }

    #[test]
    fn waiting_forge_now_lines_show_generation_progress_and_feedback() {
        let render = |lines: Vec<Line<'_>>| {
            lines
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let loading = render(waiting_forge_now_lines(Some(3), None, None));
        assert_eq!(loading, "━━━━ ━━━━ ━━━━ ━━━━ ━━━━");

        let success =
            waiting_forge_now_lines(None, Some(&QueueRegenerationFeedback::Success), None);
        assert_eq!(render(success.clone()), "✓ Forges generated");
        assert_eq!(success[0].spans[0].style.fg, Some(colors::EMBER));

        let no_safe = render(waiting_forge_now_lines(
            None,
            None,
            Some("No safe forges are available right now. Keeping current list."),
        ));
        assert!(no_safe.contains("No safe forges are available right now."));
    }

    #[test]
    fn regeneration_worker_results_update_preview_status() {
        let (success_sender, success_receiver) = std::sync::mpsc::channel();
        let mut ui = TuiState {
            queue_regeneration: Some(success_receiver),
            ..TuiState::default()
        };
        success_sender
            .send(Ok(crate::daemon::QueueRegenerationOutcome {
                source: QueueGenerationSource::LocalFallback,
                notice: Some("using local fallback".into()),
                llm_count: 0,
                local_count: 5,
            }))
            .unwrap();

        let completed_at = Instant::now();
        poll_queue_regeneration_at(&mut ui, completed_at);

        assert!(ui.queue_regeneration.is_none());
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Success)
        );
        assert_eq!(
            ui.queue_regeneration_feedback_started_at,
            Some(completed_at)
        );

        poll_queue_regeneration_at(&mut ui, completed_at + Duration::from_millis(2_999));
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Success)
        );
        poll_queue_regeneration_at(&mut ui, completed_at + Duration::from_secs(3));
        assert!(ui.queue_regeneration_feedback.is_none());
        assert!(ui.queue_regeneration_feedback_started_at.is_none());

        let (failure_sender, failure_receiver) = std::sync::mpsc::channel();
        ui.queue_regeneration = Some(failure_receiver);
        failure_sender.send(Err("failed".into())).unwrap();
        poll_queue_regeneration_at(&mut ui, completed_at + Duration::from_secs(4));
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Failure {
                no_safe_forges: false
            })
        );
        poll_queue_regeneration_at(&mut ui, completed_at + Duration::from_secs(10));
        assert!(matches!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Failure { .. })
        ));
    }

    #[test]
    fn busy_regeneration_request_is_a_true_noop() {
        let feedback = QueueRegenerationFeedback::Success;
        let feedback_started_at = Instant::now();
        let mut ui = TuiState {
            queue_regeneration_feedback: Some(feedback.clone()),
            queue_regeneration_feedback_started_at: Some(feedback_started_at),
            ..TuiState::default()
        };

        apply_queue_regeneration_start(&mut ui, QueueRegenerationStart::Busy);

        assert!(ui.queue_regeneration.is_none());
        assert!(ui.queue_regeneration_started_at.is_none());
        assert_eq!(ui.queue_regeneration_feedback, Some(feedback));
        assert_eq!(
            ui.queue_regeneration_feedback_started_at,
            Some(feedback_started_at)
        );
    }

    #[test]
    fn regeneration_loader_pulses_in_twenty_percent_steps() {
        assert_eq!(queue_regeneration_loader_frame(0), 0);
        assert_eq!(queue_regeneration_loader_frame(200), 1);
        assert_eq!(queue_regeneration_loader_frame(400), 2);
        assert_eq!(queue_regeneration_loader_frame(600), 3);
        assert_eq!(queue_regeneration_loader_frame(800), 4);
        assert_eq!(queue_regeneration_loader_frame(1_000), 5);
        assert_eq!(queue_regeneration_loader_frame(1_200), 0);

        let line = queue_regeneration_loader_line(3);
        assert_eq!(line.to_string(), "━━━━ ━━━━ ━━━━ ━━━━ ━━━━");
        assert_eq!(line.spans[0].style.fg, Some(colors::EMBER));
        assert_eq!(line.spans[2].style.fg, Some(colors::EMBER));
        assert_eq!(line.spans[4].style.fg, Some(colors::EMBER));
        assert_eq!(line.spans[6].style.fg, Some(colors::MUTED));
        assert_eq!(line.spans[8].style.fg, Some(colors::MUTED));
    }

    #[test]
    fn regeneration_shortcut_is_contextual_to_next_forges() {
        assert!(regenerate_queue_requested(
            KeyCode::Char('r'),
            ViewKind::Idle,
            true
        ));
        assert!(!regenerate_queue_requested(
            KeyCode::Char('r'),
            ViewKind::Idle,
            false
        ));
        assert!(!regenerate_queue_requested(
            KeyCode::Char('r'),
            ViewKind::Forge,
            true
        ));
    }

    #[test]
    fn compact_token_counts_use_readable_suffixes() {
        assert_eq!(compact_token_count(999), "999");
        assert_eq!(compact_token_count(1_000), "1k");
        assert_eq!(compact_token_count(1_250), "1.2k");
        assert_eq!(compact_token_count(1_000_000), "1M");
        assert_eq!(compact_token_count(1_250_000), "1.2M");
    }

    #[test]
    fn demo_title_is_visibly_marked() {
        let title = title_line(true).to_string();

        assert!(title.contains("Svarog"));
        assert!(title.contains("[demo]"));
        assert!(!title_line(false).to_string().contains("[demo]"));
    }

    #[test]
    fn recommender_line_styles_backend_in_text_color() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/config.toml".to_string(),
        };
        let line = recommender_line(&backend);

        assert_eq!(line.spans[0].content.as_ref(), "Recommender: ");
        assert_eq!(line.spans[0].style, muted());
        assert_eq!(line.spans[1].content.as_ref(), "[Codex]");
        assert_eq!(line.spans[1].style, text());
        assert_eq!(line.spans[2].content.as_ref(), "  [s] Settings");
        assert_eq!(line.spans[2].style, muted());
    }

    #[test]
    fn missing_config_uses_unknown_backend_label() {
        let root = tempdir().unwrap().keep();
        let paths = Paths::from_root(root);
        let backend = recommender_backend_view(&paths);

        assert_eq!(backend.label, "unknown");
        assert!(backend.unavailable);
    }

    #[test]
    fn backend_label_comes_from_config() {
        let root = tempdir().unwrap().keep();
        let paths = Paths::from_root(root);
        let config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };
        config::save(&paths, &config).unwrap();

        let backend = recommender_backend_view(&paths);

        assert_eq!(backend.label, "Local");
        assert!(!backend.unavailable);
    }

    #[test]
    fn backend_cycle_order_matches_tui_shortcut() {
        assert_eq!(RecommenderBackend::Local.next(), RecommenderBackend::Openai);
        assert_eq!(RecommenderBackend::Openai.next(), RecommenderBackend::Codex);
        assert_eq!(RecommenderBackend::Codex.next(), RecommenderBackend::Local);
        assert_eq!(
            RecommenderBackend::Local.previous(),
            RecommenderBackend::Codex
        );
        assert_eq!(
            RecommenderBackend::Codex.previous(),
            RecommenderBackend::Openai
        );
        assert_eq!(
            RecommenderBackend::Openai.previous(),
            RecommenderBackend::Local
        );
    }

    #[test]
    fn openai_without_api_key_is_unavailable() {
        std::env::remove_var("SVAROG_TEST_MISSING_OPENAI_KEY");
        let root = tempdir().unwrap().keep();
        let paths = Paths::from_root(root);
        let mut config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Openai,
                ..Recommender::default()
            },
            ..Config::default()
        };
        config.recommender.openai.api_key_env = "SVAROG_TEST_MISSING_OPENAI_KEY".to_string();
        config::save(&paths, &config).unwrap();

        let backend = recommender_backend_view(&paths);

        assert_eq!(backend.label, "OpenAI API");
        assert!(backend.unavailable);
    }

    #[test]
    fn unavailable_backend_lines_include_config_path() {
        let backend = BackendView {
            label: "OpenAI API".to_string(),
            unavailable: true,
            config_file: "/tmp/svarog/config.toml".to_string(),
        };
        let text = idle_lines(
            &backend,
            &ForgeActivitySummary::default(),
            &RecommenderTokenUsageByProvider::default(),
            None,
            None,
            None,
            None,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("Recommender: [OpenAI API]"));
        assert!(text.contains("Unavailable. Edit: /tmp/svarog/config.toml"));
        assert!(text.contains("Svarog OpenAI API tokens"));
        assert!(!text.contains("Svarog Codex tokens"));
        assert!(!text.contains("Use fewer Codex tokens"));
    }

    #[test]
    fn each_remote_backend_shows_only_its_own_usage() {
        let usage = RecommenderTokenUsageByProvider {
            codex: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 12_400,
                    output_tokens: 320,
                },
                week: TokenUsageTotals::default(),
            },
            openai: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 111_500,
                    output_tokens: 2_900,
                },
                week: TokenUsageTotals {
                    input_tokens: 111_500,
                    output_tokens: 2_900,
                },
            },
        };

        let openai = BackendView {
            label: "OpenAI API".into(),
            unavailable: false,
            config_file: "/tmp/svarog/config.toml".into(),
        };
        let text = recommender_usage_lines(&openai, &usage)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Svarog OpenAI API tokens (in/out)"));
        assert!(text.contains("Today  111.5k / 2.9k"));
        assert!(!text.contains("12.4k / 320"));
        assert!(!text.contains("Use fewer Codex tokens"));

        let local = BackendView {
            label: "Local".into(),
            unavailable: false,
            config_file: "/tmp/svarog/config.toml".into(),
        };
        assert!(recommender_usage_lines(&local, &usage).is_empty());
    }

    #[test]
    fn idle_lines_show_recommender_status_message() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/svarog/config.toml".to_string(),
        };
        let text = idle_lines(
            &backend,
            &ForgeActivitySummary::default(),
            &RecommenderTokenUsageByProvider::default(),
            Some("Could not update recommender. Edit: /tmp/svarog/config.toml"),
            None,
            None,
            None,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("Could not update recommender. Edit: /tmp/svarog/config.toml"));
    }

    #[test]
    fn forge_lines_show_reps_and_weight() {
        let ui = TuiState {
            recommendation_id: Some(1),
            actual_reps: 15,
            skip_check: false,
            animation_frame: 1,
            ..TuiState::default()
        };
        let text = forge_lines(&rec(), &ui)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("LEFT CURL"));
        assert!(text.contains("12 kg"));
        assert!(text.contains("10 reps"));
        assert!(text.contains("15"));
        assert!(text.contains("[i] How to"));
    }

    #[test]
    fn exercise_help_shows_numbered_instructions_and_controls() {
        let mut recommendation = rec();
        recommendation.movement_id = "Goblet_Squat".into();
        recommendation.movement_name = "Goblet Squat".into();
        let entry = exercise_catalog::find("Goblet_Squat").unwrap();

        let text = exercise_help_lines(&recommendation, Some(entry), false, None, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Goblet Squat\nHow to"));
        assert!(text.contains("1. Stand holding a light kettlebell"));
        assert!(text.contains("2. Squat down between your legs"));
        assert!(text.contains("3. At the bottom position"));
        assert!(text.contains("[o] Open images"));
        assert!(text.contains("[↑/↓] Scroll"));
        assert!(text.contains("[esc] Back"));
    }

    #[test]
    fn exercise_help_handles_missing_instructions_and_download_feedback() {
        let mut recommendation = rec();
        recommendation.movement_id = "One-Arm_Kettlebell_Swings".into();
        let entry = exercise_catalog::find("One-Arm_Kettlebell_Swings").unwrap();
        assert!(entry.instructions.is_empty());

        let loading = exercise_help_lines(&recommendation, Some(entry), true, None, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(loading.contains("No written instructions are available"));
        assert!(loading.contains("Downloading reference images…"));

        let failed = exercise_help_lines(
            &recommendation,
            Some(entry),
            false,
            Some("Could not prepare images: offline"),
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(failed.contains("Could not prepare images: offline"));
    }

    #[test]
    fn exercise_help_shortcut_is_only_active_during_a_forge() {
        assert!(exercise_help_requested(KeyCode::Char('i'), ViewKind::Forge));
        assert!(exercise_help_requested(KeyCode::Char('?'), ViewKind::Forge));
        assert!(!exercise_help_requested(KeyCode::Char('i'), ViewKind::Idle));
        assert!(!exercise_help_requested(
            KeyCode::Char('i'),
            ViewKind::Cooldown
        ));
    }

    #[test]
    fn media_worker_failure_is_reported_only_while_same_help_is_open() {
        let recommendation = rec();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut ui = TuiState {
            show_help: true,
            exercise_media: Some(receiver),
            exercise_media_id: Some(recommendation.movement_id.clone()),
            ..TuiState::default()
        };
        sender.send(Err("offline".into())).unwrap();

        poll_exercise_media(&mut ui, Some(&recommendation));

        assert!(ui.exercise_media.is_none());
        assert_eq!(ui.exercise_media_feedback.as_deref(), Some("offline"));

        let (sender, receiver) = std::sync::mpsc::channel();
        ui.show_help = false;
        ui.exercise_media = Some(receiver);
        ui.exercise_media_id = Some(recommendation.movement_id.clone());
        ui.exercise_media_feedback = None;
        sender.send(Err("offline again".into())).unwrap();

        poll_exercise_media(&mut ui, Some(&recommendation));

        assert!(ui.exercise_media_feedback.is_none());
    }

    #[test]
    fn forge_animation_cycles_quiet_anvil_and_ten_mixed_spark_bursts() {
        let render = |lines: &[Line<'_>]| {
            lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let quiet = animation_lines(0);
        let bursts = (0..SPARK_BURSTS.len())
            .map(|index| animation_lines(index * 2 + 1))
            .collect::<Vec<_>>();
        let rendered_bursts = bursts.iter().map(|lines| render(lines)).collect::<Vec<_>>();
        let unique_bursts = rendered_bursts
            .iter()
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(quiet.len(), 5);
        assert_eq!(
            render(&quiet),
            "\n\n       ___┬___\n          ▔\n[FORGING IN PROGRESS]"
        );
        assert_eq!(bursts.len(), 10);
        assert_eq!(unique_bursts.len(), 10);
        assert_eq!(
            rendered_bursts[0],
            "      ˚   ⋆   ｡\n        ✧ ⋆✧˚\n     ｡ ___┬___ ˚\n          ▔\n[FORGING IN PROGRESS]"
        );
        for (index, burst) in bursts.iter().enumerate() {
            assert_eq!(burst.len(), 5);
            assert_eq!(render(&animation_lines(index * 2)), render(&quiet));

            let spark_colors = burst
                .iter()
                .flat_map(|line| &line.spans)
                .filter(|span| ["✧", "˚", "⋆", "｡"].contains(&span.content.as_ref()))
                .filter_map(|span| span.style.fg)
                .collect::<Vec<_>>();
            assert!(spark_colors.contains(&colors::EMBER));
            assert!(spark_colors.contains(&colors::MUTED));
            assert!(!render(burst).contains(['⚒', '|', '/', '\\']));
            assert_eq!(burst[2].spans[10].content, "┬");
            assert_eq!(burst[2].spans[10].style.fg, Some(colors::EMBER));
            assert_eq!(burst[4].spans[0].style.fg, Some(colors::EMBER));
        }
        for burst in SPARK_BURSTS {
            assert!(burst.len() >= 9);
            assert!(burst.iter().filter(|(_, _, _, amber)| *amber).count() >= 4);
            assert!(burst.iter().any(|(_, _, _, amber)| !*amber));
            assert!(burst.iter().all(|(row, column, character, amber)| {
                *row < 3
                    && (4..=16).contains(column)
                    && ['✧', '˚', '⋆', '｡'].contains(character)
                    && (!*amber
                        || (*row == 1 && (8..=12).contains(column))
                        || (*row == 0 && (9..=12).contains(column)))
            }));
        }
        assert_eq!(quiet[2].spans[10].content, "┬");
        assert_eq!(quiet[2].spans[10].style.fg, Some(colors::MUTED));
        assert_eq!(quiet[4].spans[0].style.fg, Some(colors::MUTED));
        assert_eq!(render(&animation_lines(20)), render(&quiet));
    }

    #[test]
    fn tmux_control_lines_explain_focus_and_keyboard_controls() {
        let text = tmux_control_lines()
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Click pane to focus."));
        assert!(text.contains("Drag border to resize."));
        assert!(text.contains("Ctrl-b + ←/→ switches panes."));
    }

    #[test]
    fn quit_hint_is_the_last_line_for_every_screen_layout() {
        let view = ViewModel {
            kind: ViewKind::Idle,
            recommendation: None,
            backend: BackendView {
                label: "Codex".to_string(),
                unavailable: false,
                config_file: "/tmp/config.toml".to_string(),
            },
            activity: ForgeActivitySummary::default(),
            token_usage: RecommenderTokenUsageByProvider::default(),
            history: Vec::new(),
            next_forges: Vec::new(),
        };

        for in_tmux in [false, true] {
            let lines = screen_lines(&view, &TuiState::default(), in_tmux);
            assert_eq!(lines.last().unwrap().to_string(), "[q] Quit");
        }
    }

    #[test]
    fn only_lowercase_q_quits() {
        assert!(quit_requested(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        for key in [
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        ] {
            assert!(!quit_requested(key));
        }
    }

    #[test]
    fn plus_and_equals_increase_actual_reps() {
        assert!(increase_reps_requested(KeyCode::Char('+')));
        assert!(increase_reps_requested(KeyCode::Char('=')));
        assert!(!increase_reps_requested(KeyCode::Char('-')));
    }

    #[test]
    fn escape_cancels_skip_confirmation_without_selecting_a_skip() {
        assert_eq!(
            skip_confirmation_action(KeyCode::Esc),
            Some(SkipConfirmationAction::Cancel)
        );
        assert_eq!(
            skip_confirmation_action(KeyCode::Char('n')),
            Some(SkipConfirmationAction::Normal)
        );
        assert_eq!(
            skip_confirmation_action(KeyCode::Char('y')),
            Some(SkipConfirmationAction::Fatigued)
        );
        assert_eq!(
            skip_confirmation_action(KeyCode::Backspace),
            Some(SkipConfirmationAction::Remove)
        );
    }

    #[test]
    fn skip_check_asks_one_question() {
        let ui = TuiState {
            recommendation_id: Some(1),
            actual_reps: 10,
            skip_check: true,
            animation_frame: 0,
            ..TuiState::default()
        };
        let text = forge_lines(&rec(), &ui)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Are you fatigued?"));
        assert!(text.contains("[y] Yes"));
        assert!(text.contains("[n] No"));
        assert!(text.contains("[backspace] Skip and remove this exercise"));
    }
}
