use crate::cli;
use crate::config::{
    self, Config, Forge, Paths, RecommenderBackend, RuntimeEnv, RuntimeMode, UnitSystem,
};
use crate::daemon::{self, ForgeNowResult, QueueRegenerationResult, QueueRegenerationStart};
use crate::exercise_catalog::{self, ExerciseCatalogEntry};
use crate::exercise_media::{self, PreparedGallery};
use crate::fuel::{self, FuelParseOutcome};
use crate::models::{
    AppStateKind, ForgeActivitySummary, FuelEntry, FuelParseResult, NutritionTotals,
    Recommendation, RecommenderTokenProvider, RecommenderTokenUsageByProvider, SetStatus,
    WaterTotal,
};
use crate::secrets;
use crate::storage::{ForgeHistoryEntry, Store};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
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
use zeroize::Zeroizing;

type Spark = (usize, usize, char, bool);

const VIEW_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

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
    settings_regeneration: bool,
    forge_now_feedback: Option<String>,
    demo: bool,
    saved_openai_key_available: Option<bool>,
    settings: Option<SettingsState>,
    add_fuel: Option<AddFuelState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddFuelFocus {
    Meal,
    Water,
    Recent,
}

#[derive(Debug)]
struct AddFuelState {
    focus: AddFuelFocus,
    input: String,
    cursor: usize,
    parsed: Option<FuelParseOutcome>,
    parsing: Option<FuelParseJob>,
    parsing_started_at: Option<Instant>,
    next_parse_id: u64,
    scroll: usize,
    recent: Vec<FuelEntry>,
    nutrition: NutritionTotals,
    selected_recent: usize,
    confirming_delete: bool,
    water: WaterTotal,
    unit_system: UnitSystem,
    backend: RecommenderBackend,
    local_date: chrono::NaiveDate,
    feedback: Option<String>,
}

#[derive(Debug)]
struct FuelParseJob {
    id: u64,
    receiver: Receiver<(u64, Result<FuelParseOutcome, String>)>,
    cancel: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<()>,
}

struct SettingsState {
    draft: Config,
    applied_recommender_backend: RecommenderBackend,
    row: usize,
    editing: bool,
    edit_value: Zeroizing<String>,
    edit_cursor: usize,
    edit_scroll: usize,
    selecting_archetype: bool,
    custom_archetype: bool,
    archetype_original: Option<Forge>,
    saved_openai_key_present: bool,
    saved_openai_key_error: Option<String>,
    confirming_openai_key_delete: bool,
    error: Option<String>,
}

impl std::fmt::Debug for SettingsState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SettingsState")
            .field("row", &self.row)
            .field("editing", &self.editing)
            .field("edit_value", &"[REDACTED]")
            .field(
                "confirming_openai_key_delete",
                &self.confirming_openai_key_delete,
            )
            .finish_non_exhaustive()
    }
}

const SETTINGS_ROWS: usize = 16;

fn settings_row_order(settings: &SettingsState) -> Vec<usize> {
    let mut rows = Vec::with_capacity(SETTINGS_ROWS);
    rows.extend([0, 1]);
    if settings.draft.recommender.backend == RecommenderBackend::OpenaiKeyring {
        rows.push(15);
    }
    rows.extend(2..15);
    rows
}

fn move_settings_focus(settings: &mut SettingsState, forward: bool) {
    let rows = settings_row_order(settings);
    let position = rows
        .iter()
        .position(|row| *row == settings.row)
        .unwrap_or(0);
    let position = if forward {
        (position + 1).min(rows.len().saturating_sub(1))
    } else {
        position.saturating_sub(1)
    };
    settings.row = rows[position];
}

fn refresh_saved_openai_key_state(
    settings: &mut SettingsState,
    cached_availability: &mut Option<bool>,
    paths: &Paths,
) {
    refresh_saved_openai_key_state_with(settings, cached_availability, || {
        secrets::has_openai_api_key(paths)
    });
}

fn refresh_saved_openai_key_state_with(
    settings: &mut SettingsState,
    cached_availability: &mut Option<bool>,
    lookup: impl FnOnce() -> Result<bool>,
) {
    if settings.draft.recommender.backend != RecommenderBackend::OpenaiKeyring {
        return;
    }
    let result = match *cached_availability {
        Some(present) => Ok(present),
        None => lookup(),
    };
    match result {
        Ok(present) => {
            *cached_availability = Some(present);
            settings.saved_openai_key_present = present;
            settings.saved_openai_key_error = None;
        }
        Err(error) => {
            settings.saved_openai_key_present = false;
            settings.saved_openai_key_error = Some(error.to_string());
        }
    }
}

fn update_saved_openai_key_cache_after_apply(
    cached_availability: &mut Option<bool>,
    backend: RecommenderBackend,
    saved_key_available: bool,
    clear: impl FnOnce(),
) {
    if backend == RecommenderBackend::OpenaiKeyring {
        *cached_availability = Some(saved_key_available);
    } else {
        clear();
        *cached_availability = None;
    }
}

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
    nutrition: NutritionTotals,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchetypeSelectorContext {
    Onboarding,
    Settings,
}

pub fn run(env: &RuntimeEnv, shutdown: Arc<AtomicBool>) -> Result<()> {
    let saved_openai_key_available = config::load_or_default(&env.paths)
        .ok()
        .filter(|config| config.recommender.backend == RecommenderBackend::OpenaiKeyring)
        .and_then(|_| secrets::has_openai_api_key(&env.paths).ok());
    let store = Store::open(&env.paths.database_file)?;
    let mut view = load_view(&store, &env.paths, saved_openai_key_available);
    let mut last_view_refresh = Instant::now();
    let mut force_view_refresh = false;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let keyboard_enhancement = matches!(supports_keyboard_enhancement(), Ok(true));
    if keyboard_enhancement {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut ui = TuiState {
        demo: env.mode == RuntimeMode::Dev,
        saved_openai_key_available,
        ..TuiState::default()
    };
    let mut last_spark_toggle = Instant::now();
    let in_tmux = std::env::var_os("TMUX").is_some();

    let result: Result<()> = loop {
        if shutdown.load(Ordering::Acquire) {
            cancel_add_fuel(&mut ui);
            break Ok(());
        }
        if last_spark_toggle.elapsed() >= Duration::from_secs(1) {
            ui.animation_frame = (ui.animation_frame + 1) % (SPARK_BURSTS.len() * 2);
            last_spark_toggle = Instant::now();
        }

        let now = Instant::now();
        let queue_regeneration_finished = poll_queue_regeneration(&mut ui);
        poll_add_fuel(&mut ui);
        refresh_add_fuel_day(&mut ui, &store);
        if view_refresh_due(
            last_view_refresh,
            now,
            force_view_refresh || queue_regeneration_finished,
        ) {
            view = load_view(&store, &env.paths, ui.saved_openai_key_available);
            last_view_refresh = now;
            force_view_refresh = false;
        }
        if view.kind == ViewKind::Forge {
            ui.show_history = false;
            ui.show_next = false;
        }
        reconcile_add_fuel_view(&mut ui, view.kind);
        sync_reps(&mut ui, view.recommendation.as_ref());
        poll_exercise_media(&mut ui, view.recommendation.as_ref());
        terminal.draw(|frame| {
            let lines = if let Some(settings) = ui.settings.as_ref() {
                settings_lines(settings, ui.demo, frame.area().width, frame.area().height)
            } else if let Some(add_fuel) = ui.add_fuel.as_ref() {
                add_fuel_lines(add_fuel, ui.demo, frame.area().width, frame.area().height)
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
                if ui.add_fuel.is_some() {
                    let close = handle_add_fuel_key(&mut ui, key.code, key.modifiers, env, &store);
                    if close {
                        ui.add_fuel = None;
                    }
                    continue;
                }
                if ui.settings.is_some() {
                    if let Err(error) = handle_settings_key(&mut ui, key.code, key.modifiers, env) {
                        if let Some(settings) = ui.settings.as_mut() {
                            settings.error = Some(error.to_string());
                        }
                    }
                    if ui.settings.is_none() {
                        force_view_refresh = true;
                    }
                    continue;
                }
                if matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown)
                    && key.code == KeyCode::Char('s')
                {
                    if let Ok(draft) = config::load_or_default(&env.paths) {
                        let saved_openai_key_present =
                            ui.saved_openai_key_available.unwrap_or(false);
                        let applied_recommender_backend = draft.recommender.backend;
                        let mut settings = SettingsState {
                            draft,
                            applied_recommender_backend,
                            row: 0,
                            editing: false,
                            edit_value: Zeroizing::new(String::new()),
                            edit_cursor: 0,
                            edit_scroll: 0,
                            selecting_archetype: false,
                            custom_archetype: false,
                            archetype_original: None,
                            saved_openai_key_present,
                            saved_openai_key_error: None,
                            confirming_openai_key_delete: false,
                            error: None,
                        };
                        refresh_saved_openai_key_state(
                            &mut settings,
                            &mut ui.saved_openai_key_available,
                            &env.paths,
                        );
                        ui.settings = Some(settings);
                    }
                    continue;
                }
                if quit_requested(key) {
                    break Ok(());
                }
                if add_fuel_requested(key.code, view.kind, ui.show_history, ui.show_next) {
                    match open_add_fuel(env, &store) {
                        Ok(state) => ui.add_fuel = Some(state),
                        Err(error) => {
                            ui.status_message = Some(format!("Could not open Add fuel: {error}"));
                        }
                    }
                    continue;
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
                            Ok(ForgeNowResult::Started) => force_view_refresh = true,
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
                        if cli::tui_action_with_reps(env, SetStatus::Done, ui.actual_reps).is_ok() {
                            force_view_refresh = true;
                        }
                        ui.skip_check = false;
                    }
                    (KeyCode::Char('s'), ViewKind::Forge, false) => {
                        ui.skip_check = true;
                    }
                    (code, ViewKind::Forge, true) if skip_confirmation_action(code).is_some() => {
                        match skip_confirmation_action(code).unwrap() {
                            SkipConfirmationAction::Fatigued => {
                                if cli::tui_action_skip_fatigued(env).is_ok() {
                                    force_view_refresh = true;
                                }
                            }
                            SkipConfirmationAction::Normal => {
                                if cli::tui_action(env, SetStatus::Skipped).is_ok() {
                                    force_view_refresh = true;
                                }
                            }
                            SkipConfirmationAction::Remove => {
                                match cli::tui_action_remove_exercise(env) {
                                    Ok(()) => {
                                        force_view_refresh = true;
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
                        if cli::tui_action(env, SetStatus::Pain).is_ok() {
                            force_view_refresh = true;
                        }
                        ui.skip_check = false;
                    }
                    _ => {}
                }
            }
        }
    };

    cancel_add_fuel(&mut ui);

    if keyboard_enhancement {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
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

fn view_refresh_due(last_refresh: Instant, now: Instant, force: bool) -> bool {
    force || now.saturating_duration_since(last_refresh) >= VIEW_REFRESH_INTERVAL
}

fn load_view(store: &Store, paths: &Paths, saved_openai_key_available: Option<bool>) -> ViewModel {
    let backend = recommender_backend_view(paths, saved_openai_key_available);
    let state = store.state().ok();
    let recommendation = store.latest_open_recommendation().ok().flatten();
    let activity = store.completed_forge_summary().unwrap_or_default();
    let nutrition = store.nutrition_totals_today().unwrap_or_default();
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
        nutrition,
        token_usage,
        history,
        next_forges,
    }
}

fn recommender_backend_view(
    paths: &Paths,
    saved_openai_key_available: Option<bool>,
) -> BackendView {
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
        unavailable: !backend_available(&config, saved_openai_key_available),
        config_file,
    }
}

fn backend_available(config: &config::Config, saved_openai_key_available: Option<bool>) -> bool {
    match config.recommender.backend {
        RecommenderBackend::Codex => command_available(&config.recommender.codex.command),
        RecommenderBackend::OpenaiEnv => std::env::var(&config.recommender.openai.api_key_env)
            .is_ok_and(|value| !value.trim().is_empty()),
        RecommenderBackend::OpenaiKeyring => saved_openai_key_available.unwrap_or(false),
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

fn poll_queue_regeneration(ui: &mut TuiState) -> bool {
    poll_queue_regeneration_at(ui, Instant::now())
}

fn open_add_fuel(env: &RuntimeEnv, store: &Store) -> Result<AddFuelState> {
    let config = config::load_or_default(&env.paths)?;
    let now = Local::now();
    Ok(AddFuelState {
        focus: AddFuelFocus::Meal,
        input: String::new(),
        cursor: 0,
        parsed: None,
        parsing: None,
        parsing_started_at: None,
        next_parse_id: 1,
        scroll: 0,
        recent: store.recent_fuel_entries(5)?,
        nutrition: store.nutrition_totals_today()?,
        selected_recent: 0,
        confirming_delete: false,
        water: store.water_total_today()?,
        unit_system: config.profile.unit_system,
        backend: config.recommender.backend,
        local_date: now.date_naive(),
        feedback: None,
    })
}

fn poll_add_fuel(ui: &mut TuiState) {
    let Some(state) = ui.add_fuel.as_mut() else {
        return;
    };
    let active_id = state.parsing.as_ref().map(|job| job.id);
    let result = match state.parsing.as_ref() {
        Some(job) => match job.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Disconnected) => {
                Some((job.id, Err("nutrition parser stopped unexpectedly".into())))
            }
            Err(TryRecvError::Empty) => None,
        },
        None => None,
    };
    let Some(result) = result else {
        return;
    };
    if let Some(job) = state.parsing.take() {
        let _ = job.worker.join();
    }
    state.parsing_started_at = None;
    match result {
        (request_id, Ok(parsed)) if Some(request_id) == active_id => {
            state.parsed = Some(parsed);
            state.scroll = 0;
            state.feedback = None;
        }
        (request_id, Err(error)) if Some(request_id) == active_id => {
            state.feedback = Some(format!("Could not parse fuel: {error}"))
        }
        _ => {}
    }
}

fn cancel_add_fuel(ui: &mut TuiState) {
    if let Some(job) = ui.add_fuel.as_mut().and_then(|state| state.parsing.take()) {
        job.cancel.store(true, Ordering::Release);
        let _ = job.worker.join();
    }
    if let Some(state) = ui.add_fuel.as_mut() {
        state.parsing_started_at = None;
    }
    ui.add_fuel = None;
}

fn reconcile_add_fuel_view(ui: &mut TuiState, kind: ViewKind) {
    if kind == ViewKind::Forge {
        cancel_add_fuel(ui);
    }
}

fn refresh_add_fuel_day(ui: &mut TuiState, store: &Store) {
    refresh_add_fuel_day_at(ui, store, Local::now().date_naive());
}

fn refresh_add_fuel_day_at(ui: &mut TuiState, store: &Store, today: chrono::NaiveDate) {
    let Some(state) = ui.add_fuel.as_mut() else {
        return;
    };
    if state.local_date == today {
        return;
    }
    state.local_date = today;
    state.water = store.water_total_today().unwrap_or_default();
    state.recent = store.recent_fuel_entries(5).unwrap_or_default();
    state.nutrition = store.nutrition_totals_today().unwrap_or_default();
    state.selected_recent = 0;
    state.feedback = Some("Started a new local day.".into());
}

fn start_fuel_parse(state: &mut AddFuelState, env: &RuntimeEnv) {
    if state.backend == RecommenderBackend::Local {
        state.feedback = Some(
            "Meal and drink parsing needs Codex or an OpenAI backend; water is available.".into(),
        );
        return;
    }
    let input = state.input.trim().to_string();
    if input.is_empty() {
        state.feedback = Some("Describe a meal or drink first.".into());
        return;
    }
    let paths = env.paths.clone();
    let config = match config::load_or_default(&paths) {
        Ok(config) => config,
        Err(error) => {
            state.feedback = Some(format!("Could not load settings: {error}"));
            return;
        }
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let request_id = state.next_parse_id;
    state.next_parse_id = state.next_parse_id.saturating_add(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        let result = Store::open(&paths.database_file)
            .and_then(|store| fuel::parse_fuel(&store, &config, &paths, &input, worker_cancel))
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send((request_id, result));
    });
    state.parsing = Some(FuelParseJob {
        id: request_id,
        receiver,
        cancel,
        worker,
    });
    state.parsing_started_at = Some(Instant::now());
    state.feedback = None;
}

fn focus_scroll_hint(state: &AddFuelState) -> usize {
    match state.focus {
        AddFuelFocus::Meal => 0,
        AddFuelFocus::Water => 4,
        AddFuelFocus::Recent => 11,
    }
}

fn handle_add_fuel_key(
    ui: &mut TuiState,
    code: KeyCode,
    _modifiers: KeyModifiers,
    env: &RuntimeEnv,
    store: &Store,
) -> bool {
    let Some(state) = ui.add_fuel.as_mut() else {
        return false;
    };
    if state.parsing.is_some() {
        if code == KeyCode::Esc {
            if let Some(job) = state.parsing.take() {
                job.cancel.store(true, Ordering::Release);
                let _ = job.worker.join();
            }
            state.parsing_started_at = None;
            state.feedback = Some("Nutrition parsing cancelled.".into());
        }
        return false;
    }
    if let Some(outcome) = state.parsed.as_ref() {
        match code {
            KeyCode::Esc => {
                state.parsed = None;
                state.scroll = 0;
            }
            KeyCode::Up => state.scroll = state.scroll.saturating_sub(1),
            KeyCode::Down => state.scroll = state.scroll.saturating_add(1),
            KeyCode::PageUp => state.scroll = state.scroll.saturating_sub(5),
            KeyCode::PageDown => state.scroll = state.scroll.saturating_add(5),
            KeyCode::Home => state.scroll = 0,
            KeyCode::End => state.scroll = usize::MAX,
            KeyCode::Enter => {
                let result = store.save_fuel_entry(
                    state.input.trim(),
                    &outcome.parsed,
                    outcome.provider,
                    outcome.model,
                    chrono::Utc::now(),
                );
                match result {
                    Ok(_) => {
                        state.parsed = None;
                        state.input.clear();
                        state.cursor = 0;
                        state.recent = store.recent_fuel_entries(5).unwrap_or_default();
                        state.nutrition = store.nutrition_totals_today().unwrap_or_default();
                        state.selected_recent = 0;
                        state.feedback = Some("✓ Meal or drink saved".into());
                    }
                    Err(error) => state.feedback = Some(format!("Could not save fuel: {error}")),
                }
            }
            _ => {}
        }
        return false;
    }
    if state.confirming_delete {
        match code {
            KeyCode::Esc => state.confirming_delete = false,
            KeyCode::Enter | KeyCode::Char('y') => {
                if let Some(entry) = state.recent.get(state.selected_recent) {
                    match store.delete_fuel_entry(entry.id) {
                        Ok(true) => {
                            state.recent = store.recent_fuel_entries(5).unwrap_or_default();
                            state.nutrition = store.nutrition_totals_today().unwrap_or_default();
                            state.selected_recent = state
                                .selected_recent
                                .min(state.recent.len().saturating_sub(1));
                            state.feedback = Some("Fuel entry deleted.".into());
                        }
                        Ok(false) => {
                            state.feedback = Some("Fuel entry was already removed.".into())
                        }
                        Err(error) => {
                            state.feedback = Some(format!("Could not delete fuel: {error}"))
                        }
                    }
                }
                state.confirming_delete = false;
            }
            _ => {}
        }
        return false;
    }
    match code {
        KeyCode::Esc => return true,
        KeyCode::Tab => {
            state.focus = match state.focus {
                AddFuelFocus::Meal => AddFuelFocus::Water,
                AddFuelFocus::Water if state.recent.is_empty() => AddFuelFocus::Meal,
                AddFuelFocus::Water => AddFuelFocus::Recent,
                AddFuelFocus::Recent => AddFuelFocus::Meal,
            };
            state.scroll = focus_scroll_hint(state);
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                AddFuelFocus::Meal if state.recent.is_empty() => AddFuelFocus::Water,
                AddFuelFocus::Meal => AddFuelFocus::Recent,
                AddFuelFocus::Water => AddFuelFocus::Meal,
                AddFuelFocus::Recent => AddFuelFocus::Water,
            };
            state.scroll = focus_scroll_hint(state);
        }
        KeyCode::Up if state.focus == AddFuelFocus::Water => {
            state.focus = AddFuelFocus::Meal;
            state.scroll = 0;
        }
        KeyCode::Down if state.focus == AddFuelFocus::Meal => {
            state.focus = AddFuelFocus::Water;
            state.scroll = 4;
        }
        KeyCode::Down if state.focus == AddFuelFocus::Water && !state.recent.is_empty() => {
            state.focus = AddFuelFocus::Recent;
            state.scroll = focus_scroll_hint(state);
        }
        KeyCode::Enter if state.focus == AddFuelFocus::Meal => start_fuel_parse(state, env),
        KeyCode::Backspace if state.focus == AddFuelFocus::Meal => {
            backspace_at_cursor(&mut state.input, &mut state.cursor)
        }
        KeyCode::Delete if state.focus == AddFuelFocus::Meal => {
            delete_at_cursor(&mut state.input, state.cursor)
        }
        KeyCode::Left if state.focus == AddFuelFocus::Meal => {
            state.cursor = state.cursor.saturating_sub(1)
        }
        KeyCode::Right if state.focus == AddFuelFocus::Meal => {
            state.cursor = (state.cursor + 1).min(state.input.chars().count())
        }
        KeyCode::Home if state.focus == AddFuelFocus::Meal => state.cursor = 0,
        KeyCode::End if state.focus == AddFuelFocus::Meal => {
            state.cursor = state.input.chars().count()
        }
        KeyCode::Char(ch)
            if state.focus == AddFuelFocus::Meal && state.input.chars().count() < 500 =>
        {
            insert_at_cursor(&mut state.input, &mut state.cursor, ch)
        }
        KeyCode::Char('+') | KeyCode::Char('=') if state.focus == AddFuelFocus::Water => {
            adjust_water_from_tui(state, store, 1.0)
        }
        KeyCode::Char('-') if state.focus == AddFuelFocus::Water => {
            adjust_water_from_tui(state, store, -1.0)
        }
        KeyCode::Up if state.focus == AddFuelFocus::Recent => {
            state.selected_recent = state.selected_recent.saturating_sub(1)
        }
        KeyCode::Down if state.focus == AddFuelFocus::Recent => {
            state.selected_recent =
                (state.selected_recent + 1).min(state.recent.len().saturating_sub(1))
        }
        KeyCode::Char('d') if state.focus == AddFuelFocus::Recent && !state.recent.is_empty() => {
            state.confirming_delete = true
        }
        _ => {}
    }
    false
}

fn adjust_water_from_tui(state: &mut AddFuelState, store: &Store, direction: f64) {
    let step_ml = match state.unit_system {
        UnitSystem::Metric => 200.0,
        UnitSystem::Imperial => 8.0 * crate::storage::ML_PER_US_FL_OZ,
    };
    match store.adjust_water_today(direction * step_ml, state.unit_system) {
        Ok(total) => {
            state.water = total;
            state.feedback = Some("✓ Water updated".into());
        }
        Err(error) => state.feedback = Some(format!("Could not update water: {error}")),
    }
}

fn add_fuel_lines(
    state: &AddFuelState,
    demo: bool,
    area_width: u16,
    area_height: u16,
) -> Vec<Line<'static>> {
    if let Some(outcome) = state.parsed.as_ref() {
        return fuel_review_lines(
            &outcome.parsed,
            demo,
            state.feedback.as_deref(),
            area_width,
            area_height,
            state.scroll,
        );
    }
    let width = usize::from(area_width.max(1));
    let mut lines = vec![
        with_demo(Line::from(Span::styled("Add fuel", accent_bold())), demo),
        Line::from(""),
        Line::from(Span::styled("Meal or drink", text_bold())),
        Line::from(vec![
            Span::styled(
                if state.focus == AddFuelFocus::Meal {
                    "› "
                } else {
                    "  "
                },
                accent_bold(),
            ),
            Span::styled(
                if state.input.is_empty() {
                    if state.focus == AddFuelFocus::Meal {
                        "│ Describe what you ate or drank…".to_string()
                    } else {
                        "Describe what you ate or drank…".to_string()
                    }
                } else {
                    editor_window(
                        &state.input,
                        state.cursor,
                        0,
                        width.saturating_sub(3).max(1),
                    )
                },
                if state.focus == AddFuelFocus::Meal {
                    accent()
                } else {
                    text()
                },
            ),
        ]),
        Line::from(Span::styled(
            if state.backend == RecommenderBackend::Local {
                "Nutrition parsing unavailable with Local recommender"
            } else {
                "[enter] Log that fuel"
            },
            muted(),
        )),
        Line::from(""),
        Line::from(Span::styled("Plain water · today", text_bold())),
        Line::from(vec![
            Span::styled(
                if state.focus == AddFuelFocus::Water {
                    "› "
                } else {
                    "  "
                },
                accent_bold(),
            ),
            Span::styled(format_water_total(state.water, state.unit_system), accent()),
            Span::styled(
                match state.unit_system {
                    UnitSystem::Metric => "  [+/-] 200 ml",
                    UnitSystem::Imperial => "  [+/-] 8 US fl oz",
                },
                muted(),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("Today’s nutrition", text_bold())),
        nutrition_summary_line(&state.nutrition),
        Line::from(""),
        Line::from(Span::styled("Recent fuel", text_bold())),
    ];
    if state.recent.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No meals or drinks logged yet.",
            muted(),
        )));
    } else {
        for (index, entry) in state.recent.iter().enumerate() {
            let selected = state.focus == AddFuelFocus::Recent && index == state.selected_recent;
            lines.push(Line::from(vec![
                Span::styled(if selected { "› " } else { "  " }, accent_bold()),
                Span::styled(
                    clipped_text(&entry.raw_text, width.saturating_sub(18).max(4)),
                    if selected { accent() } else { text() },
                ),
                Span::styled(format!(" · {:.0} kcal", entry.totals.calories), muted()),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "[↑/↓] Select  [d] Delete",
            muted(),
        )));
    }
    lines.push(Line::from(""));
    if state.parsing.is_some() {
        let frame = state
            .parsing_started_at
            .map(|started| queue_regeneration_loader_frame(started.elapsed().as_millis()))
            .unwrap_or(0);
        lines.push(queue_regeneration_loader_line(frame));
        lines.push(Line::from(Span::styled(
            "Estimating nutrition with Luna…  [esc] Cancel",
            muted(),
        )));
    } else if state.confirming_delete {
        lines.push(Line::from(Span::styled(
            "Delete this fuel entry?",
            accent(),
        )));
        lines.push(Line::from(Span::styled(
            "[enter/y] Delete  [esc] Cancel",
            muted(),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "[tab/shift-tab/↑/↓] Move  [esc] Back",
            muted(),
        )));
    }
    if let Some(feedback) = state.feedback.as_deref() {
        lines.push(Line::from(Span::styled(feedback.to_string(), muted())));
    }
    let footer_len = if state.parsing.is_some() || state.confirming_delete {
        2 + usize::from(state.feedback.is_some())
    } else {
        1 + usize::from(state.feedback.is_some())
    };
    let footer = lines.split_off(lines.len().saturating_sub(footer_len));
    fuel_viewport(lines, footer, usize::from(area_height), state.scroll)
}

fn fuel_review_lines(
    parsed: &FuelParseResult,
    demo: bool,
    feedback: Option<&str>,
    area_width: u16,
    area_height: u16,
    scroll: usize,
) -> Vec<Line<'static>> {
    let width = usize::from(area_width.max(1));
    let mut lines = vec![
        with_demo(Line::from(Span::styled("Review fuel", accent_bold())), demo),
        Line::from(""),
    ];
    for item in &parsed.items {
        let quantity = match (item.quantity, item.unit.as_deref()) {
            (Some(quantity), Some(unit)) => format!(" · {quantity} {unit}"),
            (Some(quantity), None) => format!(" · {quantity}"),
            _ => String::new(),
        };
        lines.extend(wrapped_styled_lines(
            &format!("{}{}", item.name, quantity),
            width,
            text_bold(),
        ));
        lines.extend(wrapped_styled_lines(
            &format!(
                "  {:.0} kcal · P {:.1}g · C {:.1}g · F {:.1}g · fiber {:.1}g · sugar {:.1}g",
                item.nutrition.calories,
                item.nutrition.protein_g,
                item.nutrition.carbohydrates_g,
                item.nutrition.fat_g,
                item.nutrition.fiber_g,
                item.nutrition.sugar_g,
            ),
            width,
            text(),
        ));
        lines.extend(wrapped_styled_lines(
            &format!(
                "  sodium {:.0}mg · potassium {:.0}mg",
                item.nutrition.sodium_mg, item.nutrition.potassium_mg
            ),
            width,
            muted(),
        ));
        for assumption in &item.assumptions {
            lines.extend(wrapped_styled_lines(
                &format!("  Estimated: {assumption}"),
                width,
                muted(),
            ));
        }
        lines.push(Line::from(""));
    }
    let totals = parsed.totals();
    lines.extend(wrapped_styled_lines(
        &format!(
            "Total · {:.0} kcal · P {:.1}g · C {:.1}g · F {:.1}g",
            totals.calories, totals.protein_g, totals.carbohydrates_g, totals.fat_g
        ),
        width,
        accent_bold(),
    ));
    lines.push(Line::from(""));
    let controls = wrapped_styled_lines(
        "[↑/↓/pgup/pgdn] Scroll  [enter] Save  [esc] Edit",
        width,
        muted(),
    );
    let controls_len = controls.len();
    lines.extend(controls);
    if let Some(feedback) = feedback {
        lines.push(Line::from(Span::styled(feedback.to_string(), accent())));
    }
    let footer_len = controls_len + usize::from(feedback.is_some());
    let footer = lines.split_off(lines.len().saturating_sub(footer_len));
    fuel_viewport(lines, footer, usize::from(area_height), scroll)
}

fn wrapped_styled_lines(text_value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let chars = text_value.chars().collect::<Vec<_>>();
    let width = width.max(1);
    if chars.is_empty() {
        return vec![Line::from("")];
    }
    chars
        .chunks(width)
        .map(|chunk| Line::from(Span::styled(chunk.iter().collect::<String>(), style)))
        .collect()
}

fn fuel_viewport(
    body: Vec<Line<'static>>,
    footer: Vec<Line<'static>>,
    height: usize,
    requested_scroll: usize,
) -> Vec<Line<'static>> {
    let footer = footer.into_iter().take(height).collect::<Vec<_>>();
    let body_height = height.saturating_sub(footer.len());
    let max_scroll = body.len().saturating_sub(body_height);
    let start = requested_scroll.min(max_scroll);
    body.into_iter()
        .skip(start)
        .take(body_height)
        .chain(footer)
        .collect()
}

fn format_water_total(total: WaterTotal, unit_system: UnitSystem) -> String {
    match unit_system {
        UnitSystem::Metric => format!("{:.0} ml", total.milliliters),
        UnitSystem::Imperial => format!("{:.1} US fl oz", total.fluid_ounces),
    }
}

fn poll_queue_regeneration_at(ui: &mut TuiState, now: Instant) -> bool {
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
        return false;
    };
    ui.queue_regeneration = None;
    ui.queue_regeneration_started_at = None;
    let settings_regeneration = std::mem::take(&mut ui.settings_regeneration);
    match result {
        Ok(_) => {
            ui.queue_regeneration_feedback = Some(QueueRegenerationFeedback::Success);
            ui.queue_regeneration_feedback_started_at = Some(now);
            if settings_regeneration {
                ui.status_message = Some("Settings saved. Future forges refreshed.".into());
            }
        }
        Err(error) => {
            ui.queue_regeneration_feedback = Some(QueueRegenerationFeedback::Failure {
                no_safe_forges: error.contains(crate::daemon::NO_SAFE_FORGES_ERROR),
            });
            ui.queue_regeneration_feedback_started_at = None;
            if settings_regeneration {
                ui.status_message = Some(
                    "Settings saved. Future forges could not be refreshed; existing queue kept."
                        .into(),
                );
            }
        }
    }
    true
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
                    &view.nutrition,
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
            &view.nutrition,
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
            &view.nutrition,
            &view.token_usage,
            ui.status_message.as_deref(),
            queue_regeneration_loader(ui),
            ui.queue_regeneration_feedback.as_ref(),
            ui.forge_now_feedback.as_deref(),
            ui.demo,
        ),
    }
}

fn archetype_lines(
    forge: &Forge,
    custom_edit: Option<&str>,
    demo: bool,
    context: ArchetypeSelectorContext,
) -> Vec<Line<'static>> {
    let archetype = crate::archetypes::get(forge.archetype);
    let title = crate::archetypes::display_name(forge.archetype, forge.custom_archetype.as_deref());
    let mut lines = vec![
        with_demo(
            Line::from(Span::styled(title.to_uppercase(), accent_bold())),
            demo,
        ),
        Line::from(Span::styled(archetype.description, text())),
        Line::from(""),
    ];
    if let Some(value) = custom_edit {
        lines.push(Line::from(Span::styled("Custom archetype", text_bold())));
        lines.push(Line::from(Span::styled(format!("> {value}_"), accent())));
        lines.push(Line::from(Span::styled("[enter] Set  [esc] Back", muted())));
    } else {
        lines.push(Line::from(Span::styled(
            "←/h Previous   →/l Next   [enter] Choose   [/] Custom",
            muted(),
        )));
        lines.push(Line::from(Span::styled(
            "You can change your archetype at any time.",
            muted(),
        )));
        if context == ArchetypeSelectorContext::Settings {
            lines.push(Line::from(Span::styled("[esc] Back", muted())));
        }
    }
    lines.push(Line::from(""));
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
    lines
}

fn settings_lines(
    settings: &SettingsState,
    demo: bool,
    area_width: u16,
    area_height: u16,
) -> Vec<Line<'static>> {
    if settings.selecting_archetype {
        return fitted_modal_lines(
            archetype_lines(
                &settings.draft.forge,
                settings
                    .custom_archetype
                    .then_some(settings.edit_value.as_str()),
                demo,
                ArchetypeSelectorContext::Settings,
            ),
            usize::from(area_height),
        );
    }
    let profile = &settings.draft.profile;
    let values = [
        (
            "Forge archetype",
            format!(
                "{}  [enter] Choose",
                crate::archetypes::display_name(
                    settings.draft.forge.archetype,
                    settings.draft.forge.custom_archetype.as_deref(),
                )
            ),
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
                "Height (ft/in)"
            },
            profile
                .height_cm
                .map(|v| {
                    if profile.unit_system == UnitSystem::Metric {
                        v.to_string()
                    } else {
                        cli::format_imperial_height(v)
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
        (
            "Saved OpenAI key",
            if settings.saved_openai_key_error.is_some() {
                "credential store unavailable  [enter] Retry".into()
            } else if settings.saved_openai_key_present {
                "configured  [enter] Replace  [del] Remove".into()
            } else {
                "not set  [enter] Set".into()
            },
        ),
    ];
    let mut lines = vec![
        with_demo(Line::from(Span::styled("Settings", text_bold())), demo),
        Line::from(Span::styled(
            "↑/↓ Focus  ←/→ Adjust  Enter Open/Edit",
            muted(),
        )),
        Line::from(""),
    ];
    let footer_height = if settings.editing || settings.confirming_openai_key_delete {
        3
    } else {
        2
    } + usize::from(settings.error.is_some());
    let row_order = settings_row_order(settings);
    let visible_rows = usize::from(area_height)
        .saturating_sub(lines.len() + footer_height)
        .clamp(1, row_order.len());
    let focused_position = row_order
        .iter()
        .position(|row| *row == settings.row)
        .unwrap_or(0);
    let max_start = row_order.len().saturating_sub(visible_rows);
    let first_row = focused_position
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(max_start);
    let value_width = usize::from(area_width).saturating_sub(27).max(1);
    for index in row_order.into_iter().skip(first_row).take(visible_rows) {
        let (label, value) = &values[index];
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
            Span::styled(
                clipped_text(value, value_width),
                if selected { accent() } else { text() },
            ),
        ]));
    }
    if settings.confirming_openai_key_delete {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("Remove saved OpenAI key?", accent())),
            Line::from(Span::styled("[enter/y] Remove key  [esc] Cancel", muted())),
        ]);
    } else if settings.editing {
        let displayed_edit_value = if settings.row == 15 {
            "•".repeat(settings.edit_value.chars().count())
        } else {
            settings.edit_value.to_string()
        };
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "> {}",
                    editor_window(
                        &displayed_edit_value,
                        settings.edit_cursor,
                        settings.edit_scroll,
                        usize::from(area_width).saturating_sub(3).max(1),
                    )
                ),
                accent(),
            )),
            Line::from(Span::styled(
                if settings.row == 15 {
                    "[enter] Save key  [esc] Cancel edit"
                } else {
                    "[enter] Set field  [esc] Cancel edit"
                },
                muted(),
            )),
        ]);
    } else {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "[ctrl/cmd+s] Apply changes  [esc] Cancel settings",
                muted(),
            )),
        ]);
    }
    if let Some(error) = settings.error.as_deref() {
        lines.push(Line::from(Span::styled(error.to_string(), accent())));
    }
    lines
}

fn fitted_modal_lines(lines: Vec<Line<'static>>, height: usize) -> Vec<Line<'static>> {
    lines.into_iter().take(height.max(1)).collect()
}

fn begin_setting_edit(settings: &mut SettingsState) {
    settings.edit_value = Zeroizing::new(match settings.row {
        5 => settings
            .draft
            .profile
            .height_cm
            .map(|v| {
                if settings.draft.profile.unit_system == UnitSystem::Metric {
                    v.to_string()
                } else {
                    cli::format_imperial_height(v)
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
        15 => String::new(),
        _ => return,
    });
    settings.editing = true;
    settings.edit_cursor = settings.edit_value.chars().count();
    settings.edit_scroll = settings.edit_cursor.saturating_sub(20);
}

fn clipped_text(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".into();
    }
    format!("{}…", value.chars().take(width - 1).collect::<String>())
}

fn editor_window(value: &str, cursor: usize, requested_scroll: usize, width: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let mut start = requested_scroll.min(cursor);
    if cursor >= start.saturating_add(width.saturating_sub(1)) {
        start = cursor.saturating_add(2).saturating_sub(width);
    }
    let mut visible = chars
        .iter()
        .skip(start)
        .take(width.saturating_sub(1))
        .copied()
        .collect::<Vec<_>>();
    visible.insert(cursor.saturating_sub(start).min(visible.len()), '│');
    visible.into_iter().collect()
}

fn insert_at_cursor(value: &mut String, cursor: &mut usize, character: char) {
    let byte = value
        .char_indices()
        .nth(*cursor)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value.insert(byte, character);
    *cursor += 1;
}

fn backspace_at_cursor(value: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let mut chars = value.chars().collect::<Vec<_>>();
    chars.remove(*cursor - 1);
    *value = chars.into_iter().collect();
    *cursor -= 1;
}

fn delete_at_cursor(value: &mut String, cursor: usize) {
    let mut chars = value.chars().collect::<Vec<_>>();
    if cursor < chars.len() {
        chars.remove(cursor);
        *value = chars.into_iter().collect();
    }
}

fn adjusted_whole_value(current: Option<u32>, seed: u32, forward: bool) -> u32 {
    let value = current.unwrap_or(seed);
    if forward {
        value.saturating_add(1)
    } else {
        value.saturating_sub(1).max(1)
    }
}

fn adjust_measurement(settings: &mut SettingsState, forward: bool) {
    match settings.row {
        5 => match settings.draft.profile.unit_system {
            UnitSystem::Metric => {
                settings.draft.profile.height_cm = Some(adjusted_whole_value(
                    settings.draft.profile.height_cm,
                    170,
                    forward,
                ));
            }
            UnitSystem::Imperial => {
                let current_inches = settings
                    .draft
                    .profile
                    .height_cm
                    .map(|height| (f64::from(height) / 2.54).round() as u32);
                let inches = adjusted_whole_value(current_inches, 67, forward);
                settings.draft.profile.height_cm =
                    Some((f64::from(inches) * 2.54).round().min(f64::from(u32::MAX)) as u32);
            }
        },
        6 => {
            let metric = settings.draft.profile.unit_system == UnitSystem::Metric;
            let factor = if metric { 1.0 } else { 2.204_622_6 };
            let seed = if metric { 70.0 } else { 154.0 };
            let current = settings
                .draft
                .profile
                .weight_kg
                .filter(|weight| weight.is_finite() && *weight > 0.0)
                .map(|weight| weight * factor)
                .unwrap_or(seed);
            let adjusted = if forward {
                current + 1.0
            } else {
                (current - 1.0).max(1.0)
            };
            settings.draft.profile.weight_kg = Some(adjusted / factor);
        }
        7 => {
            settings.draft.profile.age = Some(adjusted_whole_value(
                settings.draft.profile.age,
                30,
                forward,
            ));
        }
        _ => return,
    }
    settings.error = None;
}

fn settings_apply_requested(code: KeyCode, modifiers: KeyModifiers) -> bool {
    code == KeyCode::Char('s') && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
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
            } else if settings.draft.profile.unit_system == UnitSystem::Metric {
                Some(
                    value
                        .parse::<f32>()
                        .context("height must be a number")?
                        .round() as u32,
                )
            } else {
                Some((cli::parse_imperial_height_inches(value)? as f32 * 2.54).round() as u32)
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
    settings.edit_value = Zeroizing::new(String::new());
    settings.error = None;
    Ok(())
}

fn save_openai_key_edit(settings: &mut SettingsState, paths: &Paths) -> Result<bool> {
    save_openai_key_edit_with(settings, |secret| {
        secrets::save_openai_api_key(paths, secret)
    })
}

fn save_openai_key_edit_with(
    settings: &mut SettingsState,
    save: impl FnOnce(&str) -> Result<()>,
) -> Result<bool> {
    let value = settings.edit_value.trim();
    if value.is_empty() {
        settings.error = Some("OpenAI API key cannot be empty".into());
        return Ok(false);
    }
    save(value)?;
    settings.saved_openai_key_present = true;
    settings.saved_openai_key_error = None;
    settings.editing = false;
    settings.edit_value = Zeroizing::new(String::new());
    settings.error = None;
    Ok(true)
}

fn remove_saved_openai_key(settings: &mut SettingsState, paths: &Paths) -> Result<()> {
    remove_saved_openai_key_with(settings, || secrets::remove_openai_api_key(paths))
}

fn remove_saved_openai_key_with(
    settings: &mut SettingsState,
    remove: impl FnOnce() -> Result<()>,
) -> Result<()> {
    remove()?;
    settings.saved_openai_key_present = false;
    settings.saved_openai_key_error = None;
    settings.confirming_openai_key_delete = false;
    settings.error = None;
    Ok(())
}

fn apply_settings(env: &RuntimeEnv, draft: &Config) -> Result<Receiver<QueueRegenerationResult>> {
    let previous = config::load_or_default(&env.paths)?;
    let config_existed = env.paths.config_file.exists();
    config::save(&env.paths, draft)?;
    let equipment = exercise_catalog::locally_resolved_equipment(&draft.profile.equipment_text);
    let movements = exercise_catalog::movements_for_equipment(&equipment);
    let database_result = Store::open(&env.paths.database_file)
        .and_then(|store| store.apply_user_profile_and_movement_pool(draft, &movements));
    if let Err(error) = database_result {
        let rollback = if config_existed {
            config::save(&env.paths, &previous)
        } else {
            std::fs::remove_file(&env.paths.config_file)
                .with_context(|| format!("removing {}", env.paths.config_file.display()))
        };
        return match rollback {
            Ok(()) => Err(error.context("applying settings; previous configuration restored")),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "applying settings failed: {error}; config rollback failed: {rollback_error}"
            )),
        };
    }
    Ok(daemon::regenerate_queue_after_settings(env))
}

fn handle_settings_key(
    ui: &mut TuiState,
    code: KeyCode,
    modifiers: KeyModifiers,
    env: &RuntimeEnv,
) -> Result<()> {
    let Some(settings) = ui.settings.as_mut() else {
        return Ok(());
    };
    if settings.selecting_archetype {
        settings.error = None;
        if settings.custom_archetype {
            if settings_apply_requested(code, modifiers) {
                return Ok(());
            }
            match code {
                KeyCode::Esc => {
                    settings.custom_archetype = false;
                    settings.edit_value.clear();
                    settings.edit_cursor = 0;
                    settings.edit_scroll = 0;
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
                    backspace_at_cursor(&mut settings.edit_value, &mut settings.edit_cursor);
                }
                KeyCode::Delete => delete_at_cursor(&mut settings.edit_value, settings.edit_cursor),
                KeyCode::Left => settings.edit_cursor = settings.edit_cursor.saturating_sub(1),
                KeyCode::Right => {
                    settings.edit_cursor =
                        (settings.edit_cursor + 1).min(settings.edit_value.chars().count())
                }
                KeyCode::Home => settings.edit_cursor = 0,
                KeyCode::End => settings.edit_cursor = settings.edit_value.chars().count(),
                KeyCode::Char(ch) if settings.edit_value.chars().count() < 120 => {
                    insert_at_cursor(&mut settings.edit_value, &mut settings.edit_cursor, ch)
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
                settings.edit_cursor = 0;
                settings.edit_scroll = 0;
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
    if settings.confirming_openai_key_delete {
        match code {
            KeyCode::Esc => settings.confirming_openai_key_delete = false,
            KeyCode::Enter | KeyCode::Char('y') => {
                remove_saved_openai_key(settings, &env.paths)?;
                ui.saved_openai_key_available = Some(false);
            }
            _ => {}
        }
        return Ok(());
    }
    if settings.editing {
        if settings_apply_requested(code, modifiers) {
            return Ok(());
        }
        match code {
            KeyCode::Esc => {
                settings.editing = false;
                settings.edit_value = Zeroizing::new(String::new());
            }
            KeyCode::Enter => {
                if settings.row == 15 {
                    if save_openai_key_edit(settings, &env.paths)? {
                        ui.saved_openai_key_available = Some(true);
                    }
                } else {
                    commit_setting_edit(settings)?;
                }
            }
            KeyCode::Backspace => {
                backspace_at_cursor(&mut settings.edit_value, &mut settings.edit_cursor);
            }
            KeyCode::Delete => delete_at_cursor(&mut settings.edit_value, settings.edit_cursor),
            KeyCode::Left => settings.edit_cursor = settings.edit_cursor.saturating_sub(1),
            KeyCode::Right => {
                settings.edit_cursor =
                    (settings.edit_cursor + 1).min(settings.edit_value.chars().count())
            }
            KeyCode::Home => settings.edit_cursor = 0,
            KeyCode::End => settings.edit_cursor = settings.edit_value.chars().count(),
            KeyCode::Char(ch) if settings.edit_value.chars().count() < 500 => {
                insert_at_cursor(&mut settings.edit_value, &mut settings.edit_cursor, ch)
            }
            _ => {}
        }
        return Ok(());
    }
    if settings_apply_requested(code, modifiers) {
        let saved_key_available = settings.saved_openai_key_present;
        if settings.draft.recommender.backend == RecommenderBackend::OpenaiKeyring
            && !saved_key_available
        {
            settings.error = settings.saved_openai_key_error.clone().or_else(|| {
                Some("Set a saved OpenAI key before choosing OpenAI (saved key)".into())
            });
            return Ok(());
        }
        let draft = settings.draft.clone();
        let receiver = apply_settings(env, &draft)?;
        update_saved_openai_key_cache_after_apply(
            &mut ui.saved_openai_key_available,
            draft.recommender.backend,
            saved_key_available,
            || secrets::clear_cached_openai_api_key(&env.paths),
        );
        ui.settings = None;
        ui.status_message = Some("Settings saved. Refreshing future forges…".into());
        apply_queue_regeneration_start(ui, QueueRegenerationStart::Started(receiver));
        ui.settings_regeneration = true;
        return Ok(());
    }
    match code {
        KeyCode::Esc => {
            if settings.applied_recommender_backend != RecommenderBackend::OpenaiKeyring {
                secrets::clear_cached_openai_api_key(&env.paths);
                ui.saved_openai_key_available = None;
            }
            ui.settings = None;
        }
        KeyCode::Up => move_settings_focus(settings, false),
        KeyCode::Down => move_settings_focus(settings, true),
        KeyCode::Delete if settings.row == 15 && settings.saved_openai_key_present => {
            settings.confirming_openai_key_delete = true;
            settings.error = None;
        }
        KeyCode::Left | KeyCode::Right => {
            let forward = code == KeyCode::Right;
            match settings.row {
                1 => {
                    settings.draft.recommender.backend = if forward {
                        settings.draft.recommender.backend.next()
                    } else {
                        settings.draft.recommender.backend.previous()
                    };
                    refresh_saved_openai_key_state(
                        settings,
                        &mut ui.saved_openai_key_available,
                        &env.paths,
                    );
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
                5..=7 => adjust_measurement(settings, forward),
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
        KeyCode::Enter => begin_setting_edit(settings),
        _ => {}
    }
    Ok(())
}

pub fn select_archetype(current: &Forge) -> Result<Forge> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut forge = current.clone();
    let mut custom: Option<String> = None;
    let result = loop {
        terminal.draw(|frame| {
            let paragraph = Paragraph::new(archetype_lines(
                &forge,
                custom.as_deref(),
                false,
                ArchetypeSelectorContext::Onboarding,
            ))
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
                        break Ok(forge);
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
                    KeyCode::Enter => break Ok(forge),
                    KeyCode::Esc => {}
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
        with_demo(
            Line::from(Span::styled(rec.display_name(), accent_bold())),
            demo,
        ),
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
    nutrition: &NutritionTotals,
    token_usage: &RecommenderTokenUsageByProvider,
    status_message: Option<&str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        with_demo(
            Line::from(Span::styled("Waiting for the next forge.", muted())),
            demo,
        ),
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
    lines.push(settings_control_line());
    if backend.label == RecommenderBackend::Local.label() {
        lines.push(Line::from(Span::styled(
            "Tip: Use Codex/OpenAI key recommender in Settings.",
            muted(),
        )));
    }
    lines.extend(activity_lines(activity));
    lines.extend(nutrition_lines(nutrition));
    lines.extend(recommender_usage_lines(backend, token_usage));
    if backend.unavailable {
        lines.push(Line::from(Span::styled(
            unavailable_backend_message(backend),
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
    nutrition: &NutritionTotals,
    token_usage: &RecommenderTokenUsageByProvider,
    status_message: Option<&str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        with_demo(
            Line::from(vec![
                Span::styled("Forged. ", accent_bold()),
                Span::styled("Waiting for the next forge.", muted()),
            ]),
            demo,
        ),
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
    lines.push(settings_control_line());
    lines.extend(activity_lines(activity));
    lines.extend(nutrition_lines(nutrition));
    lines.extend(recommender_usage_lines(backend, token_usage));
    if backend.unavailable {
        lines.push(Line::from(Span::styled(
            unavailable_backend_message(backend),
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

fn nutrition_lines(nutrition: &NutritionTotals) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled("Today’s nutrition:", muted())),
        nutrition_summary_line(nutrition),
    ]
}

fn nutrition_summary_line(nutrition: &NutritionTotals) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{:.0} kcal · P {:.1}g · C {:.1}g · F {:.1}g · S {:.1}g",
            nutrition.calories,
            nutrition.protein_g,
            nutrition.carbohydrates_g,
            nutrition.fat_g,
            nutrition.sugar_g,
        ),
        text(),
    ))
}

fn unavailable_backend_message(backend: &BackendView) -> String {
    if backend.label == RecommenderBackend::OpenaiKeyring.label() {
        "Unavailable. Open Settings to save an OpenAI API key.".into()
    } else {
        format!("Unavailable. Edit: {}", backend.config_file)
    }
}

fn recommender_usage_lines(
    backend: &BackendView,
    usage: &RecommenderTokenUsageByProvider,
) -> Vec<Line<'static>> {
    let (title, usage, show_api_hint) = if backend.label == RecommenderBackend::Codex.label() {
        ("Svarog Codex tokens (in/out)", &usage.codex, true)
    } else if backend.label == RecommenderBackend::OpenaiEnv.label()
        || backend.label == RecommenderBackend::OpenaiKeyring.label()
    {
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
                "Restart Svarog, then select [OpenAI (environment)] in Settings.",
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
    ])
}

fn settings_control_line() -> Line<'static> {
    Line::from(Span::styled("[s] Settings", muted()))
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

fn add_fuel_requested(
    code: KeyCode,
    kind: ViewKind,
    history_visible: bool,
    next_visible: bool,
) -> bool {
    code == KeyCode::Char('a')
        && !history_visible
        && !next_visible
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
    Line::from(Span::styled("[f] Forge now  [a] Add fuel", muted()))
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
    let mut lines = vec![with_demo(
        Line::from(Span::styled("Next forges", text_bold())),
        demo,
    )];
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
    let mut lines = vec![with_demo(
        Line::from(Span::styled("Latest forges", text_bold())),
        demo,
    )];
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
            with_demo(
                Line::from(Span::styled("Skip this forge?", accent_bold())),
                ui.demo,
            ),
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

    let mut lines = vec![with_demo(
        Line::from(Span::styled(
            rec.display_name().to_uppercase(),
            accent_bold(),
        )),
        ui.demo,
    )];
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

fn with_demo(mut line: Line<'static>, demo: bool) -> Line<'static> {
    if demo {
        line.spans.push(Span::styled("  [demo]", muted()));
    }
    line
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
        Agent, ForgeActivityTotals, FuelItem, NutritionTotals, RecommenderTokenUsage,
        RecommenderTokenUsageSummary, TokenUsageTotals,
    };
    use crate::recommender::QueueGenerationSource;
    use chrono::{TimeZone, Utc};
    use std::cell::Cell;
    use std::time::Duration;
    use tempfile::tempdir;

    fn settings_state() -> SettingsState {
        SettingsState {
            draft: Config::default(),
            applied_recommender_backend: RecommenderBackend::Local,
            row: 0,
            editing: false,
            edit_value: Zeroizing::new(String::new()),
            edit_cursor: 0,
            edit_scroll: 0,
            selecting_archetype: false,
            custom_archetype: false,
            archetype_original: None,
            saved_openai_key_present: false,
            saved_openai_key_error: None,
            confirming_openai_key_delete: false,
            error: None,
        }
    }

    fn add_fuel_state_for_test() -> AddFuelState {
        AddFuelState {
            focus: AddFuelFocus::Meal,
            input: String::new(),
            cursor: 0,
            parsed: None,
            parsing: None,
            parsing_started_at: None,
            next_parse_id: 1,
            scroll: 0,
            recent: Vec::new(),
            nutrition: NutritionTotals::default(),
            selected_recent: 0,
            confirming_delete: false,
            water: WaterTotal::default(),
            unit_system: UnitSystem::Metric,
            backend: RecommenderBackend::Codex,
            local_date: Local::now().date_naive(),
            feedback: None,
        }
    }

    #[test]
    fn view_refreshes_on_interval_or_when_forced() {
        let last_refresh = Instant::now();

        assert!(!view_refresh_due(
            last_refresh,
            last_refresh + Duration::from_millis(499),
            false
        ));
        assert!(view_refresh_due(
            last_refresh,
            last_refresh + VIEW_REFRESH_INTERVAL,
            false
        ));
        assert!(view_refresh_due(last_refresh, last_refresh, true));
    }

    #[test]
    fn settings_show_focus_and_archetype_opens_full_selector() {
        let mut settings = settings_state();
        let text = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("› Forge archetype"));
        assert!(text.contains("Athlete  [enter] Choose"));
        assert!(text.contains("Apply changes"));
        assert!(text.contains("[ctrl/cmd+s] Apply changes  [esc] Cancel settings"));
        assert!(!text.contains("› Apply changes"));

        settings.selecting_archetype = true;
        let selector = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(selector.contains("ATHLETE"));
        assert!(selector.contains("Strength"));
        assert!(selector.contains("You can change your archetype at any time."));
        assert!(selector.contains("[esc] Back"));
        assert!(!selector.contains('★'));
    }

    #[test]
    fn saved_openai_key_row_appears_below_keyring_recommender_only() {
        let mut settings = settings_state();
        let local = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert!(!local.iter().any(|line| line.contains("Saved OpenAI key")));

        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        let keyring = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let recommender = keyring
            .iter()
            .position(|line| line.contains("Recommender"))
            .unwrap();
        let saved_key = keyring
            .iter()
            .position(|line| line.contains("Saved OpenAI key"))
            .unwrap();
        let notifications = keyring
            .iter()
            .position(|line| line.contains("Notifications"))
            .unwrap();
        assert_eq!(saved_key, recommender + 1);
        assert_eq!(notifications, saved_key + 1);

        settings.row = 1;
        move_settings_focus(&mut settings, true);
        assert_eq!(settings.row, 15);
        move_settings_focus(&mut settings, true);
        assert_eq!(settings.row, 2);

        settings.saved_openai_key_present = true;
        settings.draft.recommender.backend = RecommenderBackend::Codex;
        assert!(!settings_lines(&settings, false, 120, 40)
            .iter()
            .any(|line| line.to_string().contains("Saved OpenAI key")));
        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        assert!(settings_lines(&settings, false, 120, 40)
            .iter()
            .any(|line| line.to_string().contains("configured")));
    }

    #[test]
    fn saved_key_lookup_is_lazy_cached_and_retryable() {
        let mut settings = settings_state();
        let mut cached = None;
        let lookups = Cell::new(0);

        refresh_saved_openai_key_state_with(&mut settings, &mut cached, || {
            lookups.set(lookups.get() + 1);
            Ok(true)
        });
        assert_eq!(lookups.get(), 0);
        assert_eq!(cached, None);

        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        refresh_saved_openai_key_state_with(&mut settings, &mut cached, || {
            lookups.set(lookups.get() + 1);
            Ok(true)
        });
        assert_eq!(lookups.get(), 1);
        assert_eq!(cached, Some(true));
        assert!(settings.saved_openai_key_present);

        refresh_saved_openai_key_state_with(&mut settings, &mut cached, || {
            panic!("cached availability should avoid another lookup")
        });

        cached = None;
        for expected in 2..=3 {
            refresh_saved_openai_key_state_with(&mut settings, &mut cached, || {
                lookups.set(lookups.get() + 1);
                anyhow::bail!("credential store is locked")
            });
            assert_eq!(lookups.get(), expected);
            assert_eq!(cached, None);
            assert!(settings.saved_openai_key_error.is_some());
        }
    }

    #[test]
    fn applying_another_backend_clears_cached_key_state() {
        let mut cached = Some(true);
        let cleared = Cell::new(false);
        update_saved_openai_key_cache_after_apply(
            &mut cached,
            RecommenderBackend::Codex,
            true,
            || cleared.set(true),
        );
        assert!(cleared.get());
        assert_eq!(cached, None);

        let cleared = Cell::new(false);
        update_saved_openai_key_cache_after_apply(
            &mut cached,
            RecommenderBackend::OpenaiKeyring,
            true,
            || cleared.set(true),
        );
        assert!(!cleared.get());
        assert_eq!(cached, Some(true));
    }

    #[test]
    fn onboarding_selector_puts_controls_below_description_without_escape() {
        let forge = Forge::default();
        let lines = archetype_lines(&forge, None, false, ArchetypeSelectorContext::Onboarding);

        assert_eq!(lines[0].to_string(), "ATHLETE");
        assert_eq!(lines[0].spans[0].style.fg, Some(colors::EMBER));
        assert_eq!(
            lines[1].to_string(),
            crate::archetypes::get(forge.archetype).description
        );
        assert!(lines[2].to_string().is_empty());
        assert_eq!(
            lines[3].to_string(),
            "←/h Previous   →/l Next   [enter] Choose   [/] Custom"
        );
        assert_eq!(
            lines[4].to_string(),
            "You can change your archetype at any time."
        );
        assert!(!lines.iter().any(|line| line.to_string().contains("[esc]")));
        assert_eq!(lines[6].to_string(), "Strength   ████████░░  8");
    }

    #[test]
    fn settings_text_edits_are_staged_until_apply() {
        let mut settings = settings_state();
        settings.row = 8;
        begin_setting_edit(&mut settings);
        settings.edit_value = Zeroizing::new("mobility, strength".into());
        commit_setting_edit(&mut settings).unwrap();
        assert_eq!(settings.draft.profile.goals, vec!["mobility", "strength"]);
    }

    #[test]
    fn saved_openai_key_editor_masks_and_saves_the_secret_immediately() {
        let mut settings = settings_state();
        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        settings.row = 15;
        begin_setting_edit(&mut settings);
        settings.edit_value = Zeroizing::new("sk-never-render-this".into());
        settings.edit_cursor = settings.edit_value.chars().count();

        let rendered = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("sk-never-render-this"));
        assert!(rendered.contains('•'));
        assert!(rendered.contains("[enter] Save key"));

        let mut saved = String::new();
        assert!(save_openai_key_edit_with(&mut settings, |secret| {
            saved = secret.to_string();
            Ok(())
        })
        .unwrap());
        assert_eq!(saved, "sk-never-render-this");
        assert!(settings.saved_openai_key_present);
        assert!(settings.edit_value.is_empty());
        assert!(!format!("{settings:?}").contains("sk-never-render-this"));
        assert!(!toml::to_string(&settings.draft)
            .unwrap()
            .contains("sk-never-render-this"));
    }

    #[test]
    fn saved_openai_key_removal_requires_confirmation_and_can_be_cancelled() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut settings = settings_state();
        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        settings.row = 15;
        settings.saved_openai_key_present = true;
        let mut ui = TuiState {
            settings: Some(settings),
            saved_openai_key_available: Some(true),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Delete, KeyModifiers::NONE, &env).unwrap();
        let settings = ui.settings.as_ref().unwrap();
        assert!(settings.confirming_openai_key_delete);
        let rendered = settings_lines(settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Remove saved OpenAI key?"));
        assert!(rendered.contains("[enter/y] Remove key  [esc] Cancel"));

        handle_settings_key(&mut ui, KeyCode::Esc, KeyModifiers::NONE, &env).unwrap();
        assert!(!ui.settings.as_ref().unwrap().confirming_openai_key_delete);
        assert!(ui.settings.as_ref().unwrap().saved_openai_key_present);
    }

    #[test]
    fn confirmed_saved_openai_key_removal_updates_settings_state() {
        let mut settings = settings_state();
        settings.saved_openai_key_present = true;
        settings.confirming_openai_key_delete = true;
        let mut removed = false;

        remove_saved_openai_key_with(&mut settings, || {
            removed = true;
            Ok(())
        })
        .unwrap();

        assert!(removed);
        assert!(!settings.saved_openai_key_present);
        assert!(!settings.confirming_openai_key_delete);
    }

    #[test]
    fn credential_store_failures_preserve_safe_retry_state() {
        let mut saving = settings_state();
        saving.row = 15;
        begin_setting_edit(&mut saving);
        saving.edit_value = Zeroizing::new("sk-retry-secret".into());
        saving.edit_cursor = saving.edit_value.chars().count();

        let save_error =
            save_openai_key_edit_with(&mut saving, |_| anyhow::bail!("credential store is locked"))
                .unwrap_err();
        assert_eq!(save_error.to_string(), "credential store is locked");
        assert!(saving.editing);
        assert_eq!(saving.edit_value.as_str(), "sk-retry-secret");
        assert!(!format!("{saving:?}").contains("sk-retry-secret"));

        let mut removing = settings_state();
        removing.saved_openai_key_present = true;
        removing.confirming_openai_key_delete = true;
        let remove_error = remove_saved_openai_key_with(&mut removing, || {
            anyhow::bail!("credential store is locked")
        })
        .unwrap_err();
        assert_eq!(remove_error.to_string(), "credential store is locked");
        assert!(removing.saved_openai_key_present);
        assert!(removing.confirming_openai_key_delete);
    }

    #[test]
    fn saved_key_backend_cannot_apply_without_a_key() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut settings = settings_state();
        settings.draft.recommender.backend = RecommenderBackend::OpenaiKeyring;
        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL, &env).unwrap();

        let settings = ui.settings.as_ref().unwrap();
        assert!(settings
            .error
            .as_deref()
            .unwrap()
            .contains("Set a saved OpenAI key"));
    }

    #[test]
    fn apply_shortcut_is_ignored_while_editing_a_field() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut settings = settings_state();
        settings.row = 8;
        begin_setting_edit(&mut settings);
        let original = settings.edit_value.clone();
        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL, &env).unwrap();

        let settings = ui.settings.as_ref().unwrap();
        assert!(settings.editing);
        assert_eq!(settings.edit_value, original);
    }

    #[test]
    fn settings_viewport_keeps_the_focused_row_visible() {
        let mut settings = settings_state();
        let top = settings_lines(&settings, false, 60, 10)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(top.contains("› Forge archetype"));

        settings.row = 14;
        let bottom = settings_lines(&settings, false, 60, 10)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(bottom.contains("› Exercise preferences"));
        assert!(bottom.contains("[ctrl/cmd+s] Apply changes  [esc] Cancel settings"));
        assert!(!bottom.contains("Forge archetype"));
    }

    #[test]
    fn settings_apply_shortcut_accepts_control_and_super_only() {
        assert!(settings_apply_requested(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL
        ));
        assert!(settings_apply_requested(
            KeyCode::Char('s'),
            KeyModifiers::SUPER
        ));
        assert!(!settings_apply_requested(
            KeyCode::Char('s'),
            KeyModifiers::NONE
        ));
        assert!(!settings_apply_requested(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn measurement_arrows_adjust_display_units_and_seed_unset_values() {
        let mut settings = settings_state();

        settings.row = 5;
        adjust_measurement(&mut settings, true);
        assert_eq!(settings.draft.profile.height_cm, Some(171));
        settings.draft.profile.height_cm = None;
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.height_cm, Some(169));

        settings.draft.profile.unit_system = UnitSystem::Imperial;
        settings.draft.profile.height_cm = None;
        adjust_measurement(&mut settings, true);
        assert_eq!(
            cli::format_imperial_height(settings.draft.profile.height_cm.unwrap()),
            "5'8\""
        );
        settings.draft.profile.height_cm = None;
        adjust_measurement(&mut settings, false);
        assert_eq!(
            cli::format_imperial_height(settings.draft.profile.height_cm.unwrap()),
            "5'6\""
        );

        settings.row = 6;
        settings.draft.profile.unit_system = UnitSystem::Metric;
        adjust_measurement(&mut settings, true);
        assert_eq!(settings.draft.profile.weight_kg, Some(71.0));
        settings.draft.profile.weight_kg = None;
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.weight_kg, Some(69.0));

        settings.draft.profile.unit_system = UnitSystem::Imperial;
        settings.draft.profile.weight_kg = None;
        adjust_measurement(&mut settings, true);
        assert!((settings.draft.profile.weight_kg.unwrap() * 2.204_622_6 - 155.0).abs() < 0.01);

        settings.row = 7;
        adjust_measurement(&mut settings, true);
        assert_eq!(settings.draft.profile.age, Some(31));
        settings.draft.profile.age = None;
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.age, Some(29));
    }

    #[test]
    fn measurement_arrows_saturate_at_one() {
        let mut settings = settings_state();
        settings.row = 5;
        settings.draft.profile.height_cm = Some(1);
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.height_cm, Some(1));

        settings.row = 6;
        settings.draft.profile.weight_kg = Some(1.0);
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.weight_kg, Some(1.0));

        settings.row = 7;
        settings.draft.profile.age = Some(1);
        adjust_measurement(&mut settings, false);
        assert_eq!(settings.draft.profile.age, Some(1));
    }

    #[test]
    fn editor_supports_cursor_insertion_and_deletion() {
        let mut value = "ac".to_string();
        let mut cursor = 1;
        insert_at_cursor(&mut value, &mut cursor, 'b');
        assert_eq!(value, "abc");
        assert_eq!(cursor, 2);
        backspace_at_cursor(&mut value, &mut cursor);
        assert_eq!(value, "ac");
        assert_eq!(cursor, 1);
        delete_at_cursor(&mut value, cursor);
        assert_eq!(value, "a");
        assert_eq!(editor_window("0123456789", 9, 0, 5), "678│9");
    }

    #[test]
    fn settings_accept_common_imperial_height_formats() {
        for (value, expected_cm) in [("5'11", 180), ("6 ft 1 in", 185), ("71 in", 180)] {
            let mut settings = settings_state();
            settings.row = 5;
            settings.draft.profile.unit_system = UnitSystem::Imperial;
            settings.edit_value = Zeroizing::new(value.into());
            commit_setting_edit(&mut settings).unwrap();
            assert_eq!(
                settings.draft.profile.height_cm,
                Some(expected_cm),
                "{value}"
            );
        }
    }

    #[test]
    fn settings_keys_change_only_the_focused_field_and_cancel_nested_selector() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut ui = TuiState {
            settings: Some(settings_state()),
            ..TuiState::default()
        };
        let original_archetype = ui.settings.as_ref().unwrap().draft.forge.archetype;
        handle_settings_key(&mut ui, KeyCode::Right, KeyModifiers::NONE, &env).unwrap();
        assert_eq!(
            ui.settings.as_ref().unwrap().draft.forge.archetype,
            original_archetype
        );

        handle_settings_key(&mut ui, KeyCode::Down, KeyModifiers::NONE, &env).unwrap();
        let original_backend = ui.settings.as_ref().unwrap().draft.recommender.backend;
        handle_settings_key(&mut ui, KeyCode::Right, KeyModifiers::NONE, &env).unwrap();
        assert_ne!(
            ui.settings.as_ref().unwrap().draft.recommender.backend,
            original_backend
        );

        handle_settings_key(&mut ui, KeyCode::Up, KeyModifiers::NONE, &env).unwrap();
        handle_settings_key(&mut ui, KeyCode::Enter, KeyModifiers::NONE, &env).unwrap();
        handle_settings_key(&mut ui, KeyCode::Right, KeyModifiers::NONE, &env).unwrap();
        assert_ne!(
            ui.settings.as_ref().unwrap().draft.forge.archetype,
            original_archetype
        );
        handle_settings_key(&mut ui, KeyCode::Esc, KeyModifiers::NONE, &env).unwrap();
        assert_eq!(
            ui.settings.as_ref().unwrap().draft.forge.archetype,
            original_archetype
        );
    }

    #[test]
    fn escape_discards_the_entire_settings_draft() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let original = Config::default();
        config::save(&env.paths, &original).unwrap();
        let mut settings = settings_state();
        settings.draft.profile.goals = vec!["not saved".into()];
        let mut ui = TuiState {
            settings: Some(settings),
            saved_openai_key_available: Some(true),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Esc, KeyModifiers::NONE, &env).unwrap();

        assert!(ui.settings.is_none());
        assert_eq!(ui.saved_openai_key_available, None);
        assert_eq!(
            config::load_or_default(&env.paths).unwrap().profile.goals,
            original.profile.goals
        );
    }

    #[test]
    fn cancelling_settings_keeps_cache_only_for_the_applied_saved_key_backend() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut settings = settings_state();
        settings.applied_recommender_backend = RecommenderBackend::OpenaiKeyring;
        let mut ui = TuiState {
            settings: Some(settings),
            saved_openai_key_available: Some(true),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Esc, KeyModifiers::NONE, &env).unwrap();

        assert!(ui.settings.is_none());
        assert_eq!(ui.saved_openai_key_available, Some(true));
    }

    fn test_env(root: std::path::PathBuf) -> RuntimeEnv {
        RuntimeEnv {
            mode: RuntimeMode::Dev,
            paths: Paths::from_root(root.clone()),
            codex_home: root,
            daemon_addr: "127.0.0.1:0".parse().unwrap(),
            dry_run: true,
        }
    }

    #[test]
    fn failed_database_update_restores_previous_config() {
        let root = tempdir().unwrap().keep();
        let mut env = test_env(root.clone());
        let previous = Config::default();
        config::save(&env.paths, &previous).unwrap();
        env.paths.database_file = root.join("database-directory");
        std::fs::create_dir(&env.paths.database_file).unwrap();
        let mut draft = previous.clone();
        draft.profile.goals = vec!["changed".into()];

        assert!(apply_settings(&env, &draft).is_err());
        assert_eq!(
            config::load_or_default(&env.paths).unwrap().profile.goals,
            previous.profile.goals
        );
    }

    #[test]
    fn successful_settings_apply_reports_regeneration_result() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut draft = Config::default();
        draft.recommender.backend = RecommenderBackend::Local;
        draft.profile.goals = vec!["mobility".into()];

        let receiver = apply_settings(&env, &draft).unwrap();
        assert!(receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_ok());
        assert_eq!(
            config::load_or_default(&env.paths).unwrap().profile.goals,
            vec!["mobility"]
        );
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
            &NutritionTotals::default(),
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

        assert!(!text.contains("⚒ Svarog"));
        assert!(text.contains("Waiting for the next forge."));
        assert!(text.contains("[l] Latest forges"));
        assert!(text.contains("[n] Next forges"));
        assert!(text.contains(
            "Waiting for the next forge.\n\n[f] Forge now  [a] Add fuel\n[l] Latest forges [n] Next forges\n\nRecommender: [Codex]\n[s] Settings"
        ));
        assert!(text.contains("Recommender: [Codex]\n[s] Settings"));
        assert!(!text.contains("[r] Change recommender"));
        assert!(text.contains("Completed:"));
        assert!(text.contains("Svarog Codex tokens (in/out)"));
        assert!(text.contains("Today 0 forges / 0 reps"));
        assert!(text.contains("Week 0 forges / 0 reps"));
        assert!(text.contains("Use fewer Codex tokens with an OpenAI API key"));
        assert!(text.contains("export OPENAI_API_KEY=\"...\""));
        assert!(text.contains("Restart Svarog, then select [OpenAI (environment)] in Settings."));
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
        let nutrition = NutritionTotals {
            calories: 1_840.0,
            protein_g: 122.0,
            carbohydrates_g: 190.0,
            fat_g: 61.0,
            sugar_g: 38.0,
            ..NutritionTotals::default()
        };

        for lines in [
            idle_lines(
                &backend, &activity, &nutrition, &usage, None, None, None, None, false,
            ),
            cooldown_lines(
                &backend, &activity, &nutrition, &usage, None, None, None, None, false,
            ),
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
            assert!(text.contains("Today’s nutrition:"));
            assert!(text.contains("1840 kcal · P 122.0g · C 190.0g · F 61.0g · S 38.0g"));
            assert!(text.contains("Svarog Codex tokens (in/out)"));
            assert!(text.contains("Today  12.4k / 320"));
            assert!(text.contains("Week   58.1k / 1.4k"));
            assert!(text.contains("Use fewer Codex tokens with an OpenAI API key"));
            assert!(text.find("Completed:").unwrap() < text.find("Today’s nutrition:").unwrap());
            assert!(
                text.find("Today’s nutrition:").unwrap()
                    < text.find("Svarog Codex tokens (in/out)").unwrap()
            );
        }
    }

    #[test]
    fn local_backend_hint_appears_only_on_the_idle_screen() {
        let backend = BackendView {
            label: RecommenderBackend::Local.label().into(),
            unavailable: false,
            config_file: "/tmp/config.toml".into(),
        };
        let activity = ForgeActivitySummary::default();
        let usage = RecommenderTokenUsageByProvider::default();
        let hint = "Tip: Use Codex/OpenAI key recommender in Settings.";
        let idle = idle_lines(
            &backend,
            &activity,
            &NutritionTotals::default(),
            &usage,
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
        let cooldown = cooldown_lines(
            &backend,
            &activity,
            &NutritionTotals::default(),
            &usage,
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

        assert!(idle.contains(&format!("Recommender: [Local]\n[s] Settings\n{hint}")));
        assert!(!cooldown.contains(hint));
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
            &NutritionTotals::default(),
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
                && window[2].to_string() == "[f] Forge now  [a] Add fuel"
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
            nutrition: NutritionTotals::default(),
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
            settings_regeneration: true,
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
        assert!(poll_queue_regeneration_at(&mut ui, completed_at));

        assert!(ui.queue_regeneration.is_none());
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Success)
        );
        assert_eq!(
            ui.queue_regeneration_feedback_started_at,
            Some(completed_at)
        );
        assert_eq!(
            ui.status_message.as_deref(),
            Some("Settings saved. Future forges refreshed.")
        );

        assert!(!poll_queue_regeneration_at(
            &mut ui,
            completed_at + Duration::from_millis(2_999)
        ));
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Success)
        );
        assert!(!poll_queue_regeneration_at(
            &mut ui,
            completed_at + Duration::from_secs(3)
        ));
        assert!(ui.queue_regeneration_feedback.is_none());
        assert!(ui.queue_regeneration_feedback_started_at.is_none());

        let (failure_sender, failure_receiver) = std::sync::mpsc::channel();
        ui.queue_regeneration = Some(failure_receiver);
        ui.settings_regeneration = true;
        failure_sender.send(Err("failed".into())).unwrap();
        assert!(poll_queue_regeneration_at(
            &mut ui,
            completed_at + Duration::from_secs(4)
        ));
        assert_eq!(
            ui.queue_regeneration_feedback,
            Some(QueueRegenerationFeedback::Failure {
                no_safe_forges: false
            })
        );
        assert_eq!(
            ui.status_message.as_deref(),
            Some("Settings saved. Future forges could not be refreshed; existing queue kept.")
        );
        assert!(!poll_queue_regeneration_at(
            &mut ui,
            completed_at + Duration::from_secs(10)
        ));
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
    fn demo_marker_is_appended_without_adding_a_line() {
        let marked = with_demo(Line::from("Settings"), true);
        let unmarked = with_demo(Line::from("Settings"), false);

        assert_eq!(marked.to_string(), "Settings  [demo]");
        assert_eq!(unmarked.to_string(), "Settings");
    }

    #[test]
    fn primary_screens_start_with_content_and_inline_demo_marker() {
        let backend = BackendView {
            label: "Local".into(),
            unavailable: false,
            config_file: "/tmp/config.toml".into(),
        };
        let activity = ForgeActivitySummary::default();
        let usage = RecommenderTokenUsageByProvider::default();
        let recommendation = rec();
        let ui = TuiState {
            demo: true,
            ..TuiState::default()
        };
        let screens = vec![
            idle_lines(
                &backend,
                &activity,
                &NutritionTotals::default(),
                &usage,
                None,
                None,
                None,
                None,
                true,
            ),
            cooldown_lines(
                &backend,
                &activity,
                &NutritionTotals::default(),
                &usage,
                None,
                None,
                None,
                None,
                true,
            ),
            next_forge_lines(&[], true, None, None, None),
            history_lines_for_date(&[], true, Local::now().date_naive()),
            forge_lines(&recommendation, &ui),
            exercise_help_lines(&recommendation, None, false, None, true),
            settings_lines(&settings_state(), true, 120, 40),
            archetype_lines(
                &Forge::default(),
                None,
                true,
                ArchetypeSelectorContext::Onboarding,
            ),
        ];

        for lines in screens {
            let first = lines.first().unwrap().to_string();
            assert!(!first.trim().is_empty());
            assert!(first.contains("[demo]"), "{first}");
            assert!(!first.contains("Svarog"), "{first}");
        }
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
        assert_eq!(line.spans.len(), 2);

        let settings = settings_control_line();
        assert_eq!(settings.to_string(), "[s] Settings");
        assert_eq!(settings.spans[0].style, muted());
    }

    #[test]
    fn missing_config_uses_unknown_backend_label() {
        let root = tempdir().unwrap().keep();
        let paths = Paths::from_root(root);
        let backend = recommender_backend_view(&paths, None);

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

        let backend = recommender_backend_view(&paths, None);

        assert_eq!(backend.label, "Local");
        assert!(!backend.unavailable);
    }

    #[test]
    fn persistent_view_store_observes_writes_from_another_connection() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().to_path_buf());
        config::save(&paths, &Config::default()).unwrap();
        let reader = Store::open(&paths.database_file).unwrap();

        let initial = load_view(&reader, &paths, None);
        assert_eq!(initial.token_usage.codex.today.input_tokens, 0);

        let writer = Store::open(&paths.database_file).unwrap();
        writer
            .record_recommender_token_usage(
                RecommenderTokenProvider::Codex,
                &RecommenderTokenUsage {
                    input_tokens: 42,
                    cached_input_tokens: 0,
                    output_tokens: 7,
                    reasoning_output_tokens: 0,
                },
                Utc::now(),
            )
            .unwrap();

        let refreshed = load_view(&reader, &paths, None);
        assert_eq!(refreshed.token_usage.codex.today.input_tokens, 42);
        assert_eq!(refreshed.token_usage.codex.today.output_tokens, 7);
    }

    #[test]
    fn backend_cycle_order_matches_tui_shortcut() {
        assert_eq!(
            RecommenderBackend::Local.next(),
            RecommenderBackend::OpenaiEnv
        );
        assert_eq!(
            RecommenderBackend::OpenaiEnv.next(),
            RecommenderBackend::OpenaiKeyring
        );
        assert_eq!(
            RecommenderBackend::OpenaiKeyring.next(),
            RecommenderBackend::Codex
        );
        assert_eq!(RecommenderBackend::Codex.next(), RecommenderBackend::Local);
        assert_eq!(
            RecommenderBackend::Local.previous(),
            RecommenderBackend::Codex
        );
        assert_eq!(
            RecommenderBackend::Codex.previous(),
            RecommenderBackend::OpenaiKeyring
        );
        assert_eq!(
            RecommenderBackend::OpenaiKeyring.previous(),
            RecommenderBackend::OpenaiEnv
        );
        assert_eq!(
            RecommenderBackend::OpenaiEnv.previous(),
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
                backend: RecommenderBackend::OpenaiEnv,
                ..Recommender::default()
            },
            ..Config::default()
        };
        config.recommender.openai.api_key_env = "SVAROG_TEST_MISSING_OPENAI_KEY".to_string();
        config::save(&paths, &config).unwrap();

        let backend = recommender_backend_view(&paths, None);

        assert_eq!(backend.label, "OpenAI (environment)");
        assert!(backend.unavailable);
    }

    #[test]
    fn unavailable_backend_lines_include_config_path() {
        let backend = BackendView {
            label: "OpenAI (environment)".to_string(),
            unavailable: true,
            config_file: "/tmp/svarog/config.toml".to_string(),
        };
        let text = idle_lines(
            &backend,
            &ForgeActivitySummary::default(),
            &NutritionTotals::default(),
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

        assert!(text.contains("Recommender: [OpenAI (environment)]"));
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
            label: "OpenAI (environment)".into(),
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
            &NutritionTotals::default(),
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
            nutrition: NutritionTotals::default(),
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

    #[test]
    fn add_fuel_shortcut_is_limited_to_unobscured_waiting_screens() {
        assert!(add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Idle,
            false,
            false
        ));
        assert!(add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Cooldown,
            false,
            false
        ));
        assert!(!add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Forge,
            false,
            false
        ));
        assert!(!add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Idle,
            true,
            false
        ));
        assert!(!add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Idle,
            false,
            true
        ));
    }

    #[test]
    fn add_fuel_screen_uses_selected_units_and_keeps_prompt_keys_separate() {
        let mut state = AddFuelState {
            focus: AddFuelFocus::Water,
            input: "coffee with milk".into(),
            cursor: 16,
            parsed: None,
            parsing: None,
            parsing_started_at: None,
            next_parse_id: 1,
            scroll: 0,
            recent: Vec::new(),
            nutrition: NutritionTotals {
                calories: 120.0,
                protein_g: 3.0,
                carbohydrates_g: 14.0,
                fat_g: 5.0,
                sugar_g: 8.0,
                ..NutritionTotals::default()
            },
            selected_recent: 0,
            confirming_delete: false,
            water: WaterTotal {
                milliliters: 200.0,
                fluid_ounces: 200.0 / crate::storage::ML_PER_US_FL_OZ,
            },
            unit_system: UnitSystem::Metric,
            backend: RecommenderBackend::Codex,
            local_date: Local::now().date_naive(),
            feedback: None,
        };
        let rendered = add_fuel_lines(&state, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("coffee with milk"));
        assert!(rendered.contains("[enter] Log that fuel"));
        assert!(rendered.contains("200 ml  [+/-] 200 ml"));
        assert!(
            rendered.contains("Today’s nutrition\n120 kcal · P 3.0g · C 14.0g · F 5.0g · S 8.0g")
        );
        assert!(rendered.contains("Recent fuel"));
        assert!(!rendered.contains("Today’s fuel"));
        assert!(!rendered.contains("Parse with Luna"));
        assert!(!rendered.contains("Today’s recent fuel"));
        assert!(rendered.contains("No meals or drinks logged yet."));

        state.unit_system = UnitSystem::Imperial;
        let imperial = add_fuel_lines(&state, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(imperial.contains("6.8 US fl oz  [+/-] 8 US fl oz"));
        assert_eq!(
            format_water_total(state.water, UnitSystem::Imperial),
            "6.8 US fl oz"
        );
    }

    #[test]
    fn add_fuel_water_keeps_equals_as_an_add_alias() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root.clone());
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let mut state = add_fuel_state_for_test();
        state.focus = AddFuelFocus::Water;
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };

        for key in [KeyCode::Char('+'), KeyCode::Char('='), KeyCode::Char('-')] {
            assert!(!handle_add_fuel_key(
                &mut ui,
                key,
                KeyModifiers::NONE,
                &env,
                &store,
            ));
        }

        assert_eq!(ui.add_fuel.unwrap().water.milliliters, 200.0);
    }

    #[test]
    fn fuel_review_wraps_scrolls_and_keeps_save_controls_visible() {
        let parsed = FuelParseResult {
            items: (0..20)
                .map(|index| FuelItem {
                    name: format!("A deliberately long food item number {index}"),
                    quantity: Some(1.0),
                    unit: Some("serving".into()),
                    nutrition: NutritionTotals {
                        calories: 100.0,
                        carbohydrates_g: 10.0,
                        sugar_g: 2.0,
                        ..NutritionTotals::default()
                    },
                    assumptions: vec![
                        "A long preparation assumption that must wrap on narrow terminals".into(),
                    ],
                })
                .collect(),
        };

        let first = fuel_review_lines(&parsed, false, None, 24, 8, 0);
        let last = fuel_review_lines(&parsed, false, None, 24, 8, usize::MAX);
        assert!(first.last().unwrap().to_string().contains("[enter] Save"));
        assert!(last.last().unwrap().to_string().contains("[enter] Save"));
        assert_ne!(first[0].to_string(), last[0].to_string());
        assert!(first.iter().all(|line| line.width() <= 24));
        assert!(last.iter().all(|line| line.width() <= 24));
    }

    #[test]
    fn active_forge_closes_add_fuel_and_day_rollover_refreshes_totals() {
        let root = tempdir().unwrap().keep();
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let mut state = add_fuel_state_for_test();
        state.local_date -= ChronoDuration::days(1);
        state.water.milliliters = 999.0;
        state.nutrition.calories = 999.0;
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };

        refresh_add_fuel_day_at(&mut ui, &store, Local::now().date_naive());
        assert_eq!(ui.add_fuel.as_ref().unwrap().water, WaterTotal::default());
        assert_eq!(
            ui.add_fuel.as_ref().unwrap().nutrition,
            NutritionTotals::default()
        );
        reconcile_add_fuel_view(&mut ui, ViewKind::Forge);
        assert!(ui.add_fuel.is_none());
    }
}
