use crate::cli;
use crate::config::{
    self, Config, Forge, Paths, RecommenderBackend, RuntimeEnv, RuntimeMode, UnitSystem,
};
use crate::daemon::{self, ForgeNowResult, QueueRegenerationResult, QueueRegenerationStart};
use crate::exercise_catalog::{self, ExerciseCatalogEntry};
use crate::exercise_media::{self, PreparedGallery};
use crate::fuel::{self, FuelParseOutcome};
use crate::models::{
    AppStateKind, ForgeActivitySummary, FuelEntry, NutritionTotals, Recommendation,
    RecommenderTokenProvider, RecommenderTokenUsageByProvider, SetStatus, TokenUsageTotals,
    WaterTotal,
};
use crate::secrets;
use crate::storage::{ForgeHistoryEntry, LoggedDayNutritionAverage, Store, WeightProgress};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
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
    waiting_section: WaitingSection,
    waiting_page: WaitingPage,
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
    update_requested: Option<crate::update::UpdateRequest>,
    scheduled_update_check: Option<crate::update::UpdateCheckReceiver>,
    scheduled_available_update: Option<crate::update::AvailableUpdate>,
    last_update_schedule_poll: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiOutcome {
    Quit,
    Update(crate::update::UpdateRequest),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WaitingSection {
    #[default]
    Forge,
    Fuel,
    Api,
}

impl WaitingSection {
    fn previous(self) -> Self {
        match self {
            Self::Forge => Self::Api,
            Self::Fuel => Self::Forge,
            Self::Api => Self::Fuel,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Forge => Self::Fuel,
            Self::Fuel => Self::Api,
            Self::Api => Self::Forge,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WaitingPage {
    #[default]
    Dashboard,
    Forge,
    Fuel,
    Api,
    History,
    Next,
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
    confirming_update: bool,
    development: bool,
    update_check: Option<crate::update::UpdateCheckReceiver>,
    waiting_for_scheduled_update: bool,
    available_update: Option<crate::update::AvailableUpdate>,
    update_status: Option<String>,
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
            .field("confirming_update", &self.confirming_update)
            .field("update_status", &self.update_status)
            .finish_non_exhaustive()
    }
}

const SETTINGS_ROWS: usize = 17;

fn settings_row_order(settings: &SettingsState) -> Vec<usize> {
    let mut rows = Vec::with_capacity(SETTINGS_ROWS);
    rows.extend([0, 1]);
    if settings.draft.recommender.backend == RecommenderBackend::OpenaiKeyring {
        rows.push(15);
    }
    rows.extend(2..15);
    rows.push(16);
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
    nutrition_average: Option<LoggedDayNutritionAverage>,
    weight_progress: Option<WeightProgress>,
    unit_system: UnitSystem,
    token_usage: RecommenderTokenUsageByProvider,
    history: Vec<ForgeHistoryEntry>,
    next_forges: Vec<Recommendation>,
}

#[derive(Debug, Clone)]
struct BackendView {
    label: String,
    unavailable: bool,
    #[allow(dead_code)]
    config_file: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendProvider {
    Local,
    Codex,
    OpenAi,
    Unknown,
}

impl BackendView {
    fn provider(&self) -> BackendProvider {
        if self.label == RecommenderBackend::Local.label() {
            BackendProvider::Local
        } else if self.label == RecommenderBackend::Codex.label() {
            BackendProvider::Codex
        } else if self.label == RecommenderBackend::OpenaiEnv.label()
            || self.label == RecommenderBackend::OpenaiKeyring.label()
        {
            BackendProvider::OpenAi
        } else {
            BackendProvider::Unknown
        }
    }
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

pub fn run(env: &RuntimeEnv, shutdown: Arc<AtomicBool>) -> Result<TuiOutcome> {
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
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut ui = TuiState {
        demo: env.mode == RuntimeMode::Dev,
        saved_openai_key_available,
        ..TuiState::default()
    };
    let mut last_spark_toggle = Instant::now();
    let in_tmux = std::env::var_os("TMUX").is_some();

    let result: Result<TuiOutcome> = (|| loop {
        if shutdown.load(Ordering::Acquire) {
            cancel_add_fuel(&mut ui);
            break Ok(TuiOutcome::Quit);
        }
        if last_spark_toggle.elapsed() >= Duration::from_secs(1) {
            ui.animation_frame = (ui.animation_frame + 1) % (SPARK_BURSTS.len() * 2);
            last_spark_toggle = Instant::now();
        }

        let now = Instant::now();
        let queue_regeneration_finished = poll_queue_regeneration(&mut ui);
        poll_add_fuel(&mut ui);
        poll_settings_update(&mut ui);
        maybe_start_scheduled_update(&mut ui, env);
        poll_scheduled_update(&mut ui, env);
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
            ui.waiting_page = WaitingPage::Dashboard;
        }
        reconcile_add_fuel_view(&mut ui, view.kind);
        sync_reps(&mut ui, view.recommendation.as_ref());
        poll_exercise_media(&mut ui, view.recommendation.as_ref());
        terminal.draw(|frame| {
            let lines = if scheduled_update_prompt_visible(&ui, view.kind) {
                scheduled_update_lines(ui.scheduled_available_update.as_ref().unwrap(), ui.demo)
            } else if let Some(settings) = ui.settings.as_ref() {
                settings_lines(settings, ui.demo, frame.area().width, frame.area().height)
            } else if let Some(add_fuel) = ui.add_fuel.as_ref() {
                add_fuel_lines(add_fuel, ui.demo, frame.area().width, frame.area().height)
            } else {
                screen_lines(&view, &ui, in_tmux, frame.area().width)
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
            let input_event = event::read()?;
            if let Event::Paste(value) = &input_event {
                let area = terminal.size()?;
                if ui.add_fuel.is_some() {
                    handle_add_fuel_paste(&mut ui, value, area.width, area.height);
                } else {
                    handle_settings_paste(&mut ui, value);
                }
                continue;
            }
            if let Event::Key(key) = input_event {
                if scheduled_update_prompt_visible(&ui, view.kind) {
                    match key.code {
                        KeyCode::Enter | KeyCode::Char('y') => {
                            let available = ui.scheduled_available_update.take().unwrap();
                            break Ok(TuiOutcome::Update(crate::update::UpdateRequest::Release(
                                available,
                            )));
                        }
                        KeyCode::Esc | KeyCode::Char('n') => {
                            if let Some(available) = ui.scheduled_available_update.take() {
                                if let Err(error) =
                                    crate::update::dismiss_version(&env.paths, &available.version)
                                {
                                    ui.status_message =
                                        Some(format!("Could not save update choice: {error}"));
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if ui.add_fuel.is_some() {
                    let area = terminal.size()?;
                    let close = handle_add_fuel_key(
                        &mut ui,
                        key.code,
                        key.modifiers,
                        env,
                        &store,
                        area.width,
                        area.height,
                    );
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
                    if let Some(request) = ui.update_requested.take() {
                        break Ok(TuiOutcome::Update(request));
                    }
                    continue;
                }
                if matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown)
                    && ui.waiting_page == WaitingPage::Dashboard
                    && key.code == KeyCode::Char('s')
                {
                    if let Ok(draft) = config::load_or_default(&env.paths) {
                        let saved_openai_key_present =
                            ui.saved_openai_key_available.unwrap_or(false);
                        let applied_recommender_backend = draft.recommender.backend;
                        let available_update = ui.scheduled_available_update.clone();
                        let waiting_for_scheduled_update = ui.scheduled_update_check.is_some();
                        let update_status = available_update
                            .as_ref()
                            .map(|available| {
                                format!("{} available  [enter] Install", available.version)
                            })
                            .or_else(|| {
                                waiting_for_scheduled_update.then(|| "Checking for updates…".into())
                            });
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
                            confirming_update: false,
                            development: crate::update::is_development_checkout(),
                            update_check: None,
                            waiting_for_scheduled_update,
                            available_update,
                            update_status,
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
                    break Ok(TuiOutcome::Quit);
                }
                if add_fuel_requested(key.code, view.kind, ui.waiting_page) {
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
                if regenerate_queue_requested(key.code, view.kind, ui.waiting_page) {
                    if ui.queue_regeneration.is_none() {
                        apply_queue_regeneration_start(&mut ui, daemon::regenerate_queue(env));
                    }
                    continue;
                }
                if forge_now_requested(key.code, view.kind, ui.waiting_page) {
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
                            Ok(ForgeNowResult::DailyForgeCeilingReached { completed, limit }) => {
                                ui.forge_now_feedback = Some(format!(
                                    "Daily forge ceiling reached: {completed}/{limit} completed today."
                                ));
                            }
                            Ok(ForgeNowResult::CoolingDown) => {
                                ui.forge_now_feedback = Some(
                                    "Queued forges are still cooling down. Keeping current list."
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
                if handle_waiting_navigation(&mut ui, key.code, view.kind) {
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
    })();

    cancel_add_fuel(&mut ui);

    if keyboard_enhancement {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
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

fn poll_settings_update(ui: &mut TuiState) {
    let Some(settings) = ui.settings.as_mut() else {
        return;
    };
    let Some(receiver) = settings.update_check.as_ref() else {
        return;
    };
    match receiver.try_recv() {
        Ok(Ok(Some(available))) => {
            settings.update_status =
                Some(format!("{} available  [enter] Install", available.version));
            settings.available_update = Some(available);
            settings.update_check = None;
        }
        Ok(Ok(None)) => {
            settings.update_status = Some("Up to date".into());
            settings.available_update = None;
            settings.update_check = None;
        }
        Ok(Err(error)) => {
            settings.update_status = Some(format!("Check failed: {error}"));
            settings.available_update = None;
            settings.update_check = None;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            settings.update_status = Some("Check failed: update worker stopped".into());
            settings.update_check = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
    }
}

const UPDATE_SCHEDULE_POLL_INTERVAL: Duration = Duration::from_secs(60);

fn maybe_start_scheduled_update(ui: &mut TuiState, env: &RuntimeEnv) {
    if env.mode != RuntimeMode::Production
        || crate::update::is_development_checkout()
        || ui.scheduled_update_check.is_some()
        || ui.scheduled_available_update.is_some()
        || ui
            .settings
            .as_ref()
            .is_some_and(|settings| settings.update_check.is_some())
    {
        return;
    }
    let now = Instant::now();
    if ui
        .last_update_schedule_poll
        .is_some_and(|last| now.saturating_duration_since(last) < UPDATE_SCHEDULE_POLL_INTERVAL)
    {
        return;
    }
    ui.last_update_schedule_poll = Some(now);
    match crate::update::start_scheduled_check(&env.paths) {
        Ok(Some(receiver)) => ui.scheduled_update_check = Some(receiver),
        Ok(None) => {}
        Err(error) => {
            ui.status_message = Some(format!("Could not schedule update check: {error}"));
        }
    }
}

fn poll_scheduled_update(ui: &mut TuiState, env: &RuntimeEnv) {
    let Some(receiver) = ui.scheduled_update_check.as_ref() else {
        return;
    };
    let result = match receiver.try_recv() {
        Ok(result) => result,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("update worker stopped".into()),
    };
    ui.scheduled_update_check = None;
    match result {
        Ok(Some(available)) => {
            if !crate::update::is_version_dismissed(&env.paths, &available.version) {
                ui.scheduled_available_update = Some(available.clone());
                if let Some(settings) = ui.settings.as_mut() {
                    settings.update_status =
                        Some(format!("{} available  [enter] Install", available.version));
                    settings.available_update = Some(available);
                    settings.waiting_for_scheduled_update = false;
                }
            } else if let Some(settings) = ui.settings.as_mut() {
                if settings.waiting_for_scheduled_update {
                    settings.update_status = Some("Up to date".into());
                    settings.waiting_for_scheduled_update = false;
                }
            }
        }
        Ok(None) => {
            if let Some(settings) = ui.settings.as_mut() {
                if settings.waiting_for_scheduled_update {
                    settings.update_status = Some("Up to date".into());
                    settings.waiting_for_scheduled_update = false;
                }
            }
        }
        Err(error) => {
            if let Some(settings) = ui.settings.as_mut() {
                if settings.waiting_for_scheduled_update {
                    settings.update_status = Some(format!("Check failed: {error}"));
                    settings.waiting_for_scheduled_update = false;
                }
            }
        }
    }
}

fn scheduled_update_prompt_visible(ui: &TuiState, view: ViewKind) -> bool {
    ui.scheduled_available_update.is_some()
        && ui.settings.is_none()
        && ui.add_fuel.is_none()
        && ui.waiting_page == WaitingPage::Dashboard
        && !ui.show_help
        && view != ViewKind::Forge
}

fn scheduled_update_lines(
    available: &crate::update::AvailableUpdate,
    demo: bool,
) -> Vec<Line<'static>> {
    vec![
        with_demo(Line::from(Span::styled("Svarog update", text_bold())), demo),
        Line::from(""),
        Line::from(Span::styled(
            format!("Svarog {} is available.", available.version),
            accent(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "[enter/y] Install and restart  [esc/n] Not this version",
            muted(),
        )),
    ]
}

fn view_refresh_due(last_refresh: Instant, now: Instant, force: bool) -> bool {
    force || now.saturating_duration_since(last_refresh) >= VIEW_REFRESH_INTERVAL
}

fn load_view(store: &Store, paths: &Paths, saved_openai_key_available: Option<bool>) -> ViewModel {
    let backend = recommender_backend_view(paths, saved_openai_key_available);
    let unit_system = config::load_or_default(paths)
        .map(|config| config.profile.unit_system)
        .unwrap_or(UnitSystem::Metric);
    let state = store.state().ok();
    let recommendation = store.latest_open_recommendation().ok().flatten();
    let activity = store.completed_forge_summary().unwrap_or_default();
    let nutrition = store.nutrition_totals_today().unwrap_or_default();
    let nutrition_average = store.nutrition_average_recent_logged_days().ok().flatten();
    let weight_progress = store.weight_progress().ok().flatten();
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
        nutrition_average,
        weight_progress,
        unit_system,
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

fn focus_scroll_hint(state: &AddFuelState, area_width: u16) -> usize {
    match state.focus {
        AddFuelFocus::Meal => 0,
        AddFuelFocus::Water => {
            3 + editor_visual_line_count(&state.input, editor_content_width(area_width))
        }
        AddFuelFocus::Recent => {
            10 + editor_visual_line_count(&state.input, editor_content_width(area_width))
                + state.selected_recent
        }
    }
}

fn handle_add_fuel_key(
    ui: &mut TuiState,
    code: KeyCode,
    _modifiers: KeyModifiers,
    env: &RuntimeEnv,
    store: &Store,
    area_width: u16,
    area_height: u16,
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
                let result =
                    store.save_fuel_batch(&outcome.events, outcome.provider, outcome.model);
                match result {
                    Ok(ids) => {
                        let meal_count = ids.len();
                        state.parsed = None;
                        state.input.clear();
                        state.cursor = 0;
                        state.recent = store.recent_fuel_entries(5).unwrap_or_default();
                        state.nutrition = store.nutrition_totals_today().unwrap_or_default();
                        state.selected_recent = 0;
                        state.feedback = Some(format!(
                            "✓ {meal_count} {} saved",
                            if meal_count == 1 { "meal" } else { "meals" }
                        ));
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
            state.scroll = focus_scroll_hint(state, area_width);
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                AddFuelFocus::Meal if state.recent.is_empty() => AddFuelFocus::Water,
                AddFuelFocus::Meal => AddFuelFocus::Recent,
                AddFuelFocus::Water => AddFuelFocus::Meal,
                AddFuelFocus::Recent => AddFuelFocus::Water,
            };
            state.scroll = focus_scroll_hint(state, area_width);
        }
        KeyCode::Up if state.focus == AddFuelFocus::Water => {
            state.focus = AddFuelFocus::Meal;
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Down if state.focus == AddFuelFocus::Water && !state.recent.is_empty() => {
            state.focus = AddFuelFocus::Recent;
            state.scroll = focus_scroll_hint(state, area_width);
        }
        KeyCode::Enter if state.focus == AddFuelFocus::Meal => start_fuel_parse(state, env),
        KeyCode::Backspace if state.focus == AddFuelFocus::Meal => {
            backspace_at_cursor(&mut state.input, &mut state.cursor);
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Delete if state.focus == AddFuelFocus::Meal => {
            delete_at_cursor(&mut state.input, state.cursor);
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Left if state.focus == AddFuelFocus::Meal => {
            state.cursor = state.cursor.saturating_sub(1);
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Right if state.focus == AddFuelFocus::Meal => {
            state.cursor = (state.cursor + 1).min(state.input.chars().count());
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Up if state.focus == AddFuelFocus::Meal => {
            state.cursor = move_cursor_vertical(
                &state.input,
                state.cursor,
                editor_content_width(area_width),
                false,
            );
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Down if state.focus == AddFuelFocus::Meal => {
            state.cursor = move_cursor_vertical(
                &state.input,
                state.cursor,
                editor_content_width(area_width),
                true,
            );
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Home if state.focus == AddFuelFocus::Meal => {
            state.cursor = current_line_start(&state.input, state.cursor);
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::End if state.focus == AddFuelFocus::Meal => {
            state.cursor = current_line_end(&state.input, state.cursor);
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Char(ch) if state.focus == AddFuelFocus::Meal => {
            insert_fuel_text(state, &ch.to_string());
            keep_meal_cursor_visible(state, area_width, area_height);
        }
        KeyCode::Char('+') | KeyCode::Char('=') if state.focus == AddFuelFocus::Water => {
            adjust_water_from_tui(state, store, 1.0)
        }
        KeyCode::Char('-') if state.focus == AddFuelFocus::Water => {
            adjust_water_from_tui(state, store, -1.0)
        }
        KeyCode::Up if state.focus == AddFuelFocus::Recent => {
            state.selected_recent = state.selected_recent.saturating_sub(1);
            state.scroll = focus_scroll_hint(state, area_width);
        }
        KeyCode::Down if state.focus == AddFuelFocus::Recent => {
            state.selected_recent =
                (state.selected_recent + 1).min(state.recent.len().saturating_sub(1));
            state.scroll = focus_scroll_hint(state, area_width);
        }
        KeyCode::Char('d') if state.focus == AddFuelFocus::Recent && !state.recent.is_empty() => {
            state.confirming_delete = true
        }
        _ => {}
    }
    false
}

fn handle_add_fuel_paste(ui: &mut TuiState, value: &str, area_width: u16, area_height: u16) {
    let Some(state) = ui.add_fuel.as_mut() else {
        return;
    };
    if state.focus != AddFuelFocus::Meal
        || state.parsing.is_some()
        || state.parsed.is_some()
        || state.confirming_delete
    {
        return;
    }
    let normalized = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .chars()
        .filter(|character| *character == '\n' || !character.is_control())
        .collect::<String>();
    insert_fuel_text(state, &normalized);
    keep_meal_cursor_visible(state, area_width, area_height);
}

fn handle_settings_paste(ui: &mut TuiState, value: &str) {
    let Some(settings) = ui.settings.as_mut() else {
        return;
    };
    let limit: usize = if settings.selecting_archetype && settings.custom_archetype {
        120
    } else if settings.editing {
        500
    } else {
        return;
    };
    let remaining = limit.saturating_sub(settings.edit_value.chars().count());
    let accepted = value
        .chars()
        .filter(|character| !character.is_control())
        .take(remaining)
        .collect::<String>();
    insert_text_at_cursor(
        &mut settings.edit_value,
        &mut settings.edit_cursor,
        &accepted,
    );
}

fn insert_fuel_text(state: &mut AddFuelState, value: &str) {
    let remaining = fuel::MAX_FUEL_INPUT_CHARS.saturating_sub(state.input.chars().count());
    let accepted = value.chars().take(remaining).collect::<String>();
    let truncated = accepted.chars().count() < value.chars().count();
    insert_text_at_cursor(&mut state.input, &mut state.cursor, &accepted);
    state.feedback = truncated.then(|| {
        format!(
            "Meal description is limited to {} characters.",
            fuel::MAX_FUEL_INPUT_CHARS
        )
    });
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
            outcome,
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
        Line::from(Span::styled("Meals or drinks", text_bold())),
    ];
    lines.extend(meal_editor_lines(state, area_width));
    lines.extend([
        Line::from(Span::styled(
            if state.backend == RecommenderBackend::Local {
                format!(
                    "Nutrition parsing unavailable with Local recommender · {}/{}",
                    state.input.chars().count(),
                    fuel::MAX_FUEL_INPUT_CHARS
                )
            } else {
                format!(
                    "[enter] Log that fuel · {}/{}",
                    state.input.chars().count(),
                    fuel::MAX_FUEL_INPUT_CHARS
                )
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
            Span::styled(format_water_total(state.water, state.unit_system), text()),
            Span::styled(
                match state.unit_system {
                    UnitSystem::Metric => "  [+/-] 200 ml",
                    UnitSystem::Imperial => "  [+/-] 8 US fl oz",
                },
                muted(),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("Today’s fuel", text_bold())),
        nutrition_summary_line(&state.nutrition),
        Line::from(""),
        Line::from(Span::styled("Recent fuel", text_bold())),
    ]);
    if state.recent.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No meals or drinks logged yet.",
            muted(),
        )));
    } else {
        let today = Local::now().date_naive();
        for (index, entry) in state.recent.iter().enumerate() {
            let selected = state.focus == AddFuelFocus::Recent && index == state.selected_recent;
            let calories = format!(" · {:.0} kcal", entry.totals.calories);
            lines.push(Line::from(vec![
                Span::styled(if selected { "› " } else { "  " }, accent_bold()),
                Span::styled(
                    clipped_text(
                        &recent_fuel_label(entry, today),
                        width.saturating_sub(calories.chars().count() + 2).max(4),
                    ),
                    if selected { accent() } else { text() },
                ),
                Span::styled(calories, muted()),
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
    let feedback_lines = state
        .feedback
        .as_deref()
        .map(|feedback| wrapped_styled_lines(feedback, width, muted()))
        .unwrap_or_default();
    let feedback_len = feedback_lines.len();
    lines.extend(feedback_lines);
    let footer_len = if state.parsing.is_some() || state.confirming_delete {
        2 + feedback_len
    } else {
        1 + feedback_len
    };
    let footer = lines.split_off(lines.len().saturating_sub(footer_len));
    fuel_viewport(lines, footer, usize::from(area_height), state.scroll)
}

fn recent_fuel_label(entry: &FuelEntry, today: chrono::NaiveDate) -> String {
    let consumed_at = entry.created_at.with_timezone(&Local);
    let date = history_date_label(consumed_at.date_naive(), today);
    let mut items = entry
        .parsed
        .items
        .iter()
        .take(2)
        .map(|item| match (item.quantity, item.unit.as_deref()) {
            (Some(quantity), Some(unit)) => format!("{quantity} {unit} {}", item.name),
            (Some(quantity), None) => format!("{quantity} {}", item.name),
            _ => item.name.clone(),
        })
        .collect::<Vec<_>>();
    if entry.parsed.items.len() > items.len() {
        items.push(format!("+{} more", entry.parsed.items.len() - items.len()));
    }
    format!(
        "{date} {} · {}",
        consumed_at.format("%H:%M"),
        items.join(", ")
    )
}

fn fuel_review_lines(
    outcome: &FuelParseOutcome,
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
    if outcome.inferred_yesterday {
        lines.extend(wrapped_styled_lines(
            "Date inferred as yesterday because every stated meal time is later than now.",
            width,
            muted(),
        ));
        lines.push(Line::from(""));
    }
    let today = Local::now().date_naive();
    for event in &outcome.events {
        let consumed_at = event.consumed_at.with_timezone(&Local);
        lines.push(Line::from(Span::styled(
            format!(
                "{} · {}",
                history_date_label(consumed_at.date_naive(), today),
                consumed_at.format("%H:%M")
            ),
            accent_bold(),
        )));
        lines.push(Line::from(""));
        for item in &event.parsed.items {
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
    }
    let totals = outcome.totals();
    lines.extend(wrapped_styled_lines(
        &format!(
            "Total · {:.0} kcal · P {:.1}g · C {:.1}g · F {:.1}g",
            totals.calories, totals.protein_g, totals.carbohydrates_g, totals.fat_g
        ),
        width,
        accent_bold(),
    ));
    lines.push(Line::from(""));
    let meal_count = outcome.events.len();
    let controls = wrapped_styled_lines(
        &format!(
            "[↑/↓/pgup/pgdn] Scroll  [enter] Save {meal_count} {}  [esc] Edit",
            if meal_count == 1 { "meal" } else { "meals" }
        ),
        width,
        muted(),
    );
    let controls_len = controls.len();
    lines.extend(controls);
    let feedback_lines = feedback
        .map(|feedback| wrapped_styled_lines(feedback, width, accent()))
        .unwrap_or_default();
    let feedback_len = feedback_lines.len();
    lines.extend(feedback_lines);
    let footer_len = controls_len + feedback_len;
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

fn view_lines(view: &ViewModel, ui: &TuiState, area_width: u16) -> Vec<Line<'static>> {
    if matches!(view.kind, ViewKind::Idle | ViewKind::Cooldown) {
        match ui.waiting_page {
            WaitingPage::History => return history_lines(&view.history, ui.demo),
            WaitingPage::Next => {
                return next_forge_lines(
                    &view.next_forges,
                    ui.demo,
                    queue_regeneration_loader(ui),
                    ui.queue_regeneration_feedback.as_ref(),
                    ui.forge_now_feedback.as_deref(),
                );
            }
            WaitingPage::Forge => {
                return forge_detail_lines(
                    &view.activity,
                    ui.status_message.as_deref(),
                    queue_regeneration_loader(ui),
                    ui.queue_regeneration_feedback.as_ref(),
                    ui.forge_now_feedback.as_deref(),
                    ui.demo,
                );
            }
            WaitingPage::Fuel => {
                return fuel_detail_lines(
                    &view.nutrition,
                    view.nutrition_average.as_ref(),
                    view.weight_progress,
                    view.unit_system,
                    ui.demo,
                );
            }
            WaitingPage::Api => {
                return api_detail_lines(&view.backend, &view.token_usage, ui.demo);
            }
            WaitingPage::Dashboard => {}
        }
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
    let lines = match view.kind {
        ViewKind::Forge => view
            .recommendation
            .as_ref()
            .map(|rec| forge_lines(rec, ui))
            .unwrap_or_else(|| idle_lines(waiting_dashboard(view, ui, area_width), ui.demo)),
        ViewKind::Cooldown => cooldown_lines(waiting_dashboard(view, ui, area_width), ui.demo),
        ViewKind::Idle => idle_lines(waiting_dashboard(view, ui, area_width), ui.demo),
    };
    lines
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
    settings_lines_with_notification_reason(
        settings,
        demo,
        area_width,
        area_height,
        crate::notifications::unavailable_reason(),
    )
}

fn settings_lines_with_notification_reason(
    settings: &SettingsState,
    demo: bool,
    area_width: u16,
    area_height: u16,
    notification_unavailable_reason: Option<&str>,
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
            notification_setting_value(
                settings.draft.preferences.desktop_notifications,
                notification_unavailable_reason,
            ),
        ),
        (
            "Daily forge ceiling",
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
        (
            "Svarog version",
            settings.update_status.clone().unwrap_or_else(|| {
                format!(
                    "{}  [enter] {}",
                    crate::update::current_version_label(settings.development),
                    if settings.development {
                        "Rebuild checkout"
                    } else {
                        "Check for updates"
                    }
                )
            }),
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
    let footer_height = if settings.editing
        || settings.confirming_openai_key_delete
        || settings.confirming_update
    {
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
    if settings.confirming_update {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                if settings.development {
                    "Rebuild and restart the development checkout?"
                } else {
                    "Install the available update and restart Svarog?"
                },
                accent(),
            )),
            Line::from(Span::styled(
                "Unapplied settings will be discarded. [enter/y] Continue  [esc] Cancel",
                muted(),
            )),
        ]);
    } else if settings.confirming_openai_key_delete {
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

fn notification_setting_value(enabled: bool, unavailable_reason: Option<&str>) -> String {
    if !enabled {
        "disabled".to_string()
    } else if let Some(reason) = unavailable_reason {
        format!("enabled · ⚠ {}", reason)
    } else {
        "enabled".to_string()
    }
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

fn editor_content_width(area_width: u16) -> usize {
    usize::from(area_width).saturating_sub(3).max(1)
}

#[derive(Clone, Copy)]
struct EditorPosition {
    row: usize,
    character_column: usize,
    display_column: usize,
}

fn editor_layout(value: &str, width: usize) -> (Vec<String>, Vec<EditorPosition>) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut positions = vec![EditorPosition {
        row: 0,
        character_column: 0,
        display_column: 0,
    }];
    let mut row = 0;
    let mut character_column: usize = 0;
    let mut display_column: usize = 0;
    let mut soft_wrapped = false;
    for character in value.chars() {
        if character == '\n' {
            if soft_wrapped {
                soft_wrapped = false;
            } else {
                row += 1;
                character_column = 0;
                display_column = 0;
                rows.push(String::new());
            }
        } else {
            soft_wrapped = false;
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if display_column > 0 && display_column.saturating_add(character_width) > width {
                row += 1;
                character_column = 0;
                display_column = 0;
                rows.push(String::new());
            }
            rows[row].push(character);
            character_column += 1;
            display_column += character_width;
            if display_column == width {
                row += 1;
                character_column = 0;
                display_column = 0;
                rows.push(String::new());
                soft_wrapped = true;
            }
        }
        positions.push(EditorPosition {
            row,
            character_column,
            display_column,
        });
    }
    (rows, positions)
}

fn editor_visual_line_count(value: &str, width: usize) -> usize {
    editor_layout(value, width).0.len().max(1)
}

fn meal_editor_lines(state: &AddFuelState, area_width: u16) -> Vec<Line<'static>> {
    let focused = state.focus == AddFuelFocus::Meal;
    if state.input.is_empty() {
        return vec![Line::from(vec![
            Span::styled(if focused { "› " } else { "  " }, accent_bold()),
            Span::styled(
                if focused {
                    "│ Describe one meal or a whole day…"
                } else {
                    "Describe one meal or a whole day…"
                },
                if focused { accent() } else { text() },
            ),
        ])];
    }
    let (mut rows, positions) = editor_layout(&state.input, editor_content_width(area_width));
    if focused {
        let position = positions[state.cursor.min(positions.len().saturating_sub(1))];
        let byte = rows[position.row]
            .char_indices()
            .nth(position.character_column)
            .map(|(index, _)| index)
            .unwrap_or(rows[position.row].len());
        rows[position.row].insert(byte, '│');
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            Line::from(vec![
                Span::styled(
                    if focused && index == 0 { "› " } else { "  " },
                    accent_bold(),
                ),
                Span::styled(row, if focused { accent() } else { text() }),
            ])
        })
        .collect()
}

fn move_cursor_vertical(value: &str, cursor: usize, width: usize, down: bool) -> usize {
    let (_, positions) = editor_layout(value, width);
    let cursor = cursor.min(positions.len().saturating_sub(1));
    let position = positions[cursor];
    let target_row = if down {
        position.row.saturating_add(1)
    } else if position.row == 0 {
        return cursor;
    } else {
        position.row - 1
    };
    positions
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.row == target_row)
        .min_by_key(|(_, candidate)| candidate.display_column.abs_diff(position.display_column))
        .map(|(index, _)| index)
        .unwrap_or(cursor)
}

fn current_line_start(value: &str, cursor: usize) -> usize {
    value
        .chars()
        .take(cursor)
        .collect::<Vec<_>>()
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1)
}

fn current_line_end(value: &str, cursor: usize) -> usize {
    let chars = value.chars().collect::<Vec<_>>();
    chars
        .iter()
        .enumerate()
        .skip(cursor.min(chars.len()))
        .find(|(_, character)| **character == '\n')
        .map_or(chars.len(), |(index, _)| index)
}

fn keep_meal_cursor_visible(state: &mut AddFuelState, area_width: u16, area_height: u16) {
    let (_, positions) = editor_layout(&state.input, editor_content_width(area_width));
    let position = positions[state.cursor.min(positions.len().saturating_sub(1))];
    let cursor_body_row = 3 + position.row;
    let body_height = usize::from(area_height).saturating_sub(2).max(1);
    state.scroll = cursor_body_row
        .saturating_add(1)
        .saturating_sub(body_height);
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

fn insert_text_at_cursor(value: &mut String, cursor: &mut usize, text_value: &str) {
    let byte = value
        .char_indices()
        .nth(*cursor)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value.insert_str(byte, text_value);
    *cursor += text_value.chars().count();
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

/// Applies Settings synchronously. Changing the recommender backend is the only
/// Settings change that replaces the future-forge queue; all other changes keep
/// compatible queued work in place.
fn apply_settings(
    env: &RuntimeEnv,
    draft: &Config,
) -> Result<Option<Receiver<QueueRegenerationResult>>> {
    let previous = config::load_or_default(&env.paths)?;
    let recommender_changed = previous.recommender.backend != draft.recommender.backend;
    let config_existed = env.paths.config_file.exists();
    config::save(&env.paths, draft)?;
    let equipment = exercise_catalog::locally_resolved_equipment(&draft.profile.equipment_text);
    let movements = exercise_catalog::movements_for_equipment(&equipment);
    let equipment_filter = serde_json::to_string(&crate::recommender::normalize_equipment(
        &draft.profile.equipment_text,
    ))?;
    let database_result = Store::open(&env.paths.database_file).and_then(|store| {
        store.apply_user_profile_and_movement_pool(
            draft,
            previous.profile.weight_kg,
            &movements,
            &equipment_filter,
        )
    });
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
    Ok(recommender_changed.then(|| daemon::regenerate_queue_after_settings(env)))
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
    if settings.confirming_update {
        match code {
            KeyCode::Esc => settings.confirming_update = false,
            KeyCode::Enter | KeyCode::Char('y') => {
                ui.update_requested = Some(if settings.development {
                    crate::update::UpdateRequest::Development
                } else if let Some(available) = settings.available_update.clone() {
                    crate::update::UpdateRequest::Release(available)
                } else {
                    settings.confirming_update = false;
                    return Ok(());
                });
                ui.settings = None;
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
        let regeneration = apply_settings(env, &draft)?;
        update_saved_openai_key_cache_after_apply(
            &mut ui.saved_openai_key_available,
            draft.recommender.backend,
            saved_key_available,
            || secrets::clear_cached_openai_api_key(&env.paths),
        );
        ui.settings = None;
        if let Some(receiver) = regeneration {
            ui.status_message = Some("Settings saved. Refreshing future forges…".into());
            apply_queue_regeneration_start(ui, QueueRegenerationStart::Started(receiver));
            ui.settings_regeneration = true;
        } else {
            ui.status_message = Some("Settings saved.".into());
        }
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
        KeyCode::Enter if settings.row == 16 => {
            settings.error = None;
            if settings.development || settings.available_update.is_some() {
                settings.confirming_update = true;
            } else if settings.update_check.is_none() {
                settings.update_status = Some("Checking for updates…".into());
                if ui.scheduled_update_check.is_some() {
                    settings.waiting_for_scheduled_update = true;
                } else {
                    settings.update_check = Some(crate::update::start_manual_check(&env.paths)?);
                }
            }
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

fn screen_lines(
    view: &ViewModel,
    ui: &TuiState,
    in_tmux: bool,
    area_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = view_lines(view, ui, area_width);
    if in_tmux {
        lines.extend(tmux_control_lines());
    }
    if view.kind == ViewKind::Forge {
        lines.extend(quit_control_lines());
    }
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
    let rendered_height = screen_lines(view, ui, in_tmux, area_width)
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

struct WaitingDashboard<'a> {
    backend: &'a BackendView,
    activity: &'a ForgeActivitySummary,
    nutrition: &'a NutritionTotals,
    nutrition_average: Option<&'a LoggedDayNutritionAverage>,
    weight_progress: Option<WeightProgress>,
    unit_system: UnitSystem,
    token_usage: &'a RecommenderTokenUsageByProvider,
    focused: WaitingSection,
    status_message: Option<&'a str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&'a QueueRegenerationFeedback>,
    forge_now_feedback: Option<&'a str>,
    area_width: u16,
}

fn waiting_dashboard<'a>(
    view: &'a ViewModel,
    ui: &'a TuiState,
    area_width: u16,
) -> WaitingDashboard<'a> {
    WaitingDashboard {
        backend: &view.backend,
        activity: &view.activity,
        nutrition: &view.nutrition,
        nutrition_average: view.nutrition_average.as_ref(),
        weight_progress: view.weight_progress,
        unit_system: view.unit_system,
        token_usage: &view.token_usage,
        focused: ui.waiting_section,
        status_message: ui.status_message.as_deref(),
        queue_loader_frame: queue_regeneration_loader(ui),
        queue_feedback: ui.queue_regeneration_feedback.as_ref(),
        forge_now_feedback: ui.forge_now_feedback.as_deref(),
        area_width,
    }
}

fn idle_lines(data: WaitingDashboard<'_>, demo: bool) -> Vec<Line<'static>> {
    waiting_dashboard_lines(
        with_demo(
            Line::from(Span::styled("Waiting for the next forge.", muted())),
            demo,
        ),
        data,
    )
}

fn cooldown_lines(data: WaitingDashboard<'_>, demo: bool) -> Vec<Line<'static>> {
    waiting_dashboard_lines(
        with_demo(
            Line::from(vec![
                Span::styled("Forged. ", accent_bold()),
                Span::styled("Waiting for the next forge.", muted()),
            ]),
            demo,
        ),
        data,
    )
}

fn waiting_dashboard_lines(state: Line<'static>, data: WaitingDashboard<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![state, Line::from(""), forge_now_control_line()];
    lines.extend(waiting_forge_now_lines(
        data.queue_loader_frame,
        data.queue_feedback,
        data.forge_now_feedback,
    ));
    lines.push(Line::from(""));
    lines.push(waiting_summary_line(
        WaitingSection::Forge,
        data.focused,
        forge_summary_text(data.activity, data.area_width),
    ));
    lines.push(waiting_summary_line(
        WaitingSection::Fuel,
        data.focused,
        fuel_summary_text(
            data.nutrition,
            data.nutrition_average,
            data.weight_progress,
            data.unit_system,
            data.area_width,
        ),
    ));
    lines.push(waiting_summary_line(
        WaitingSection::Api,
        data.focused,
        api_summary_text(data.backend, data.token_usage, data.area_width),
    ));
    lines.extend([
        Line::from(""),
        recommender_status_line(data.backend),
        Line::from(""),
        Line::from(Span::styled("↑↓ Select   Enter Open", muted())),
        Line::from(Span::styled("[s] Settings   [q] Quit", muted())),
    ]);
    if let Some(message) = data.status_message {
        lines.push(Line::from(Span::styled(message.to_string(), muted())));
    }
    lines
}

fn waiting_summary_line(
    section: WaitingSection,
    focused: WaitingSection,
    summary: String,
) -> Line<'static> {
    let marker = if section == focused { "> " } else { "  " };
    let label = match section {
        WaitingSection::Forge => "Forge",
        WaitingSection::Fuel => "Fuel",
        WaitingSection::Api => "API",
    };
    let label_style = if section == focused {
        accent_bold()
    } else {
        text()
    };
    Line::from(vec![
        Span::styled(
            marker,
            if section == focused {
                accent()
            } else {
                muted()
            },
        ),
        Span::styled(format!("{label:<8}"), label_style),
        Span::styled(summary, text()),
    ])
}

fn forge_summary_text(activity: &ForgeActivitySummary, area_width: u16) -> String {
    let full = format!(
        "{} today · {} reps · {} this week",
        activity.today.forges, activity.today.reps, activity.week.forges
    );
    fit_summary(full, area_width, || {
        format!(
            "{} today · {} week",
            activity.today.forges, activity.week.forges
        )
    })
}

fn fuel_summary_text(
    today: &NutritionTotals,
    average: Option<&LoggedDayNutritionAverage>,
    progress: Option<WeightProgress>,
    unit_system: UnitSystem,
    area_width: u16,
) -> String {
    let nutrition = if has_nutrition(today) {
        Some(today)
    } else {
        average.map(|value| &value.totals)
    };
    let Some(nutrition) = nutrition else {
        return "No recent data".into();
    };
    let compact = format!(
        "{:.0} kcal · {:.0}P",
        nutrition.calories, nutrition.protein_g
    );
    let full = weight_trend_text(progress, unit_system)
        .map(|weight| format!("{compact} · {weight}"))
        .unwrap_or_else(|| compact.clone());
    fit_summary(full, area_width, || compact)
}

fn api_summary_text(
    backend: &BackendView,
    usage: &RecommenderTokenUsageByProvider,
    area_width: u16,
) -> String {
    if backend.provider() == BackendProvider::OpenAi {
        let today_cost = api_cost(usage.openai.today);
        let week_cost = api_cost(usage.openai.week);
        let full = format!("{} today · {} week", today_cost, week_cost);
        fit_summary(full, area_width, || {
            format!("{today_cost} · {week_cost} week")
        })
    } else if backend.provider() == BackendProvider::Codex {
        let full = format!(
            "{} in · {} out today",
            compact_token_count(usage.codex.today.input_tokens),
            compact_token_count(usage.codex.today.output_tokens)
        );
        fit_summary(full, area_width, || {
            format!(
                "{}/{} today",
                compact_token_count(usage.codex.today.input_tokens),
                compact_token_count(usage.codex.today.output_tokens)
            )
        })
    } else {
        "No remote usage".into()
    }
}

fn fit_summary(full: String, area_width: u16, compact: impl FnOnce() -> String) -> String {
    const PREFIX_WIDTH: usize = 10;
    if PREFIX_WIDTH + full.width() <= usize::from(area_width) {
        full
    } else {
        compact()
    }
}

fn has_nutrition(nutrition: &NutritionTotals) -> bool {
    nutrition.values().iter().any(|value| *value > 0.0)
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

fn forge_detail_lines(
    activity: &ForgeActivitySummary,
    status_message: Option<&str>,
    queue_loader_frame: Option<usize>,
    queue_feedback: Option<&QueueRegenerationFeedback>,
    forge_now_feedback: Option<&str>,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        with_demo(Line::from(Span::styled("Forge", text_bold())), demo),
        Line::from(""),
    ];
    lines.extend(forge_period_lines("Today", activity.today));
    lines.push(Line::from(""));
    lines.extend(forge_period_lines("This week", activity.week));
    lines.extend([
        Line::from(""),
        forge_list_controls_line(),
        Line::from(Span::styled("[f] Forge now", muted())),
    ]);
    lines.extend(waiting_forge_now_lines(
        queue_loader_frame,
        queue_feedback,
        forge_now_feedback,
    ));
    if let Some(message) = status_message {
        lines.push(Line::from(Span::styled(message.to_string(), muted())));
    }
    lines.push(detail_footer_line());
    lines
}

fn forge_period_lines(
    label: &str,
    totals: crate::models::ForgeActivityTotals,
) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(label.to_string(), muted())),
        Line::from(Span::styled(format!("  {} forges", totals.forges), text())),
        Line::from(Span::styled(format!("  {} reps", totals.reps), text())),
    ]
}

fn fuel_detail_lines(
    today: &NutritionTotals,
    average: Option<&LoggedDayNutritionAverage>,
    progress: Option<WeightProgress>,
    unit_system: UnitSystem,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        with_demo(Line::from(Span::styled("Fuel", text_bold())), demo),
        Line::from(""),
        Line::from(Span::styled("Today", muted())),
    ];
    lines.extend(nutrition_detail_lines(today));
    if let Some(average) = average {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                format!("{}-day average", average.logged_days),
                muted(),
            )),
        ]);
        lines.extend(nutrition_detail_lines(&average.totals));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Weight", muted())));
    lines.push(Line::from(Span::styled(
        weight_trend_text(progress, unit_system).unwrap_or_else(|| "  No trend yet".into()),
        text(),
    )));
    lines.extend([
        Line::from(""),
        Line::from(Span::styled("[a] Add fuel", muted())),
        detail_footer_line(),
    ]);
    lines
}

fn nutrition_detail_lines(nutrition: &NutritionTotals) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("  {:.0} kcal", nutrition.calories),
            text(),
        )),
        Line::from(Span::styled(
            format!("  Protein    {:.0} g", nutrition.protein_g),
            text(),
        )),
        Line::from(Span::styled(
            format!("  Carbs      {:.0} g", nutrition.carbohydrates_g),
            text(),
        )),
        Line::from(Span::styled(
            format!("  Fat        {:.0} g", nutrition.fat_g),
            text(),
        )),
        Line::from(Span::styled(
            format!("  Sugar      {:.0} g", nutrition.sugar_g),
            text(),
        )),
    ]
}

fn api_detail_lines(
    backend: &BackendView,
    usage: &RecommenderTokenUsageByProvider,
    demo: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        with_demo(Line::from(Span::styled("API usage", text_bold())), demo),
        Line::from(""),
    ];
    let remote_usage = match backend.provider() {
        BackendProvider::OpenAi => Some(("OpenAI", &usage.openai, true)),
        BackendProvider::Codex => Some(("Codex", &usage.codex, false)),
        BackendProvider::Local | BackendProvider::Unknown => None,
    };
    if let Some((provider, usage, show_cost)) = remote_usage {
        lines.push(Line::from(Span::styled(provider, muted())));
        lines.push(Line::from(""));
        lines.extend(api_period_lines("Today", usage.today, show_cost));
        lines.push(Line::from(""));
        lines.extend(api_period_lines("This week", usage.week, show_cost));
    } else {
        let message = if backend.provider() == BackendProvider::Local {
            "No remote usage."
        } else {
            "Usage unavailable."
        };
        lines.push(Line::from(Span::styled(message, muted())));
    }
    lines.extend([Line::from(""), detail_footer_line()]);
    lines
}

fn api_period_lines(label: &str, totals: TokenUsageTotals, show_cost: bool) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(label.to_string(), muted())),
        Line::from(Span::styled(
            format!("  Input      {}", compact_token_count(totals.input_tokens)),
            text(),
        )),
        Line::from(Span::styled(
            format!("  Output     {}", compact_token_count(totals.output_tokens)),
            text(),
        )),
    ];
    lines.push(Line::from(Span::styled(
        if show_cost {
            format!("  Cost       {}", api_cost(totals))
        } else {
            "  Cost       unavailable".into()
        },
        text(),
    )));
    lines
}

fn detail_footer_line() -> Line<'static> {
    Line::from(Span::styled("[Esc] Back   [q] Quit", muted()))
}

fn api_cost(totals: TokenUsageTotals) -> String {
    let cost = totals.input_tokens as f64 * 0.20_f64 / 1_000_000.0
        + totals.output_tokens as f64 * 1.20_f64 / 1_000_000.0;
    format!("${cost:.2}")
}

fn recommender_status_line(backend: &BackendView) -> Line<'static> {
    let provider = match backend.provider() {
        BackendProvider::OpenAi => "OpenAI",
        BackendProvider::Codex => "Codex",
        BackendProvider::Local => "Local",
        BackendProvider::Unknown => "Recommender",
    };
    if backend.unavailable {
        Line::from(Span::styled(
            format!("⚠ {provider} unavailable"),
            accent_bold(),
        ))
    } else {
        Line::from(Span::styled(format!("{provider} · ready"), muted()))
    }
}

struct WeightTrend {
    arrow: &'static str,
    change: f32,
    unit: &'static str,
}

fn weight_trend_text(progress: Option<WeightProgress>, unit_system: UnitSystem) -> Option<String> {
    let progress = progress?;
    let trend = weight_trend(progress, unit_system)?;
    Some(format!(
        "{} {:.1} {}",
        trend.arrow, trend.change, trend.unit
    ))
}

fn weight_trend(progress: WeightProgress, unit_system: UnitSystem) -> Option<WeightTrend> {
    let factor = if unit_system == UnitSystem::Metric {
        1.0
    } else {
        2.204_622_6
    };
    let delta = (progress.current_kg - progress.starting_kg) * factor;
    if delta.abs() < 0.05 * factor {
        return None;
    }
    let (change, arrow) = if delta < 0.0 {
        (-delta, "↓")
    } else {
        (delta, "↑")
    };
    let unit = if unit_system == UnitSystem::Metric {
        "kg"
    } else {
        "lb"
    };
    Some(WeightTrend {
        arrow,
        change,
        unit,
    })
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

fn handle_waiting_navigation(ui: &mut TuiState, code: KeyCode, kind: ViewKind) -> bool {
    if !matches!(kind, ViewKind::Idle | ViewKind::Cooldown) {
        return false;
    }
    match (ui.waiting_page, code) {
        (WaitingPage::Dashboard, KeyCode::Up) => {
            ui.waiting_section = ui.waiting_section.previous();
        }
        (WaitingPage::Dashboard, KeyCode::Down) => {
            ui.waiting_section = ui.waiting_section.next();
        }
        (WaitingPage::Dashboard, KeyCode::Enter) => {
            ui.waiting_page = match ui.waiting_section {
                WaitingSection::Forge => WaitingPage::Forge,
                WaitingSection::Fuel => WaitingPage::Fuel,
                WaitingSection::Api => WaitingPage::Api,
            };
        }
        (WaitingPage::Forge, KeyCode::Char('l')) => ui.waiting_page = WaitingPage::History,
        (WaitingPage::Forge, KeyCode::Char('n')) => ui.waiting_page = WaitingPage::Next,
        (WaitingPage::History | WaitingPage::Next, KeyCode::Esc) => {
            ui.waiting_page = WaitingPage::Forge;
            ui.waiting_section = WaitingSection::Forge;
        }
        (WaitingPage::Forge | WaitingPage::Fuel | WaitingPage::Api, KeyCode::Esc) => {
            ui.waiting_page = WaitingPage::Dashboard
        }
        _ => return false,
    }
    true
}

fn forge_now_requested(code: KeyCode, kind: ViewKind, page: WaitingPage) -> bool {
    code == KeyCode::Char('f')
        && matches!(kind, ViewKind::Idle | ViewKind::Cooldown)
        && matches!(
            page,
            WaitingPage::Dashboard | WaitingPage::Forge | WaitingPage::Next
        )
}

fn add_fuel_requested(code: KeyCode, kind: ViewKind, page: WaitingPage) -> bool {
    code == KeyCode::Char('a')
        && matches!(kind, ViewKind::Idle | ViewKind::Cooldown)
        && matches!(page, WaitingPage::Dashboard | WaitingPage::Fuel)
}

fn regenerate_queue_requested(code: KeyCode, kind: ViewKind, page: WaitingPage) -> bool {
    code == KeyCode::Char('r')
        && page == WaitingPage::Next
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
                "Recommender returned no compatible forges."
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
                    "Recommender returned no compatible forges. Keeping current list."
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
    lines.push(detail_footer_line());
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
    lines.extend([Line::from(""), detail_footer_line()]);
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
        Agent, ForgeActivityTotals, FuelItem, FuelParseResult, NutritionTotals,
        RecommenderTokenUsage, RecommenderTokenUsageSummary, TimedFuelEvent, TokenUsageTotals,
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
            confirming_update: false,
            development: false,
            update_check: None,
            waiting_for_scheduled_update: false,
            available_update: None,
            update_status: None,
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
    fn settings_version_row_routes_updates_after_confirmation() {
        let root = tempdir().unwrap();
        let env = test_env(root.path().to_path_buf());
        let mut settings = settings_state();
        settings.row = 16;
        settings.development = true;
        let rendered = settings_lines(&settings, false, 120, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Svarog version"));
        assert!(rendered.contains("Rebuild checkout"));

        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };
        handle_settings_key(&mut ui, KeyCode::Enter, KeyModifiers::NONE, &env).unwrap();
        assert!(ui.settings.as_ref().unwrap().confirming_update);
        assert!(ui.update_requested.is_none());

        handle_settings_key(&mut ui, KeyCode::Enter, KeyModifiers::NONE, &env).unwrap();
        assert!(ui.settings.is_none());
        assert_eq!(
            ui.update_requested,
            Some(crate::update::UpdateRequest::Development)
        );
    }

    #[test]
    fn settings_update_check_reports_available_release_without_closing_tui() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Ok(Some(crate::update::AvailableUpdate {
                version: semver::Version::new(0, 6, 3),
                tag: "v0.6.3".into(),
            })))
            .unwrap();
        let mut settings = settings_state();
        settings.row = 16;
        settings.update_check = Some(receiver);
        settings.update_status = Some("Checking for updates…".into());
        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };

        poll_settings_update(&mut ui);

        let settings = ui.settings.as_ref().unwrap();
        assert_eq!(
            settings.update_status.as_deref(),
            Some("0.6.3 available  [enter] Install")
        );
        assert_eq!(settings.available_update.as_ref().unwrap().tag, "v0.6.3");
        assert!(settings.update_check.is_none());
    }

    #[test]
    fn scheduled_update_is_shared_with_settings_and_deferred_during_forges() {
        let root = tempdir().unwrap();
        let env = test_env(root.path().to_path_buf());
        let (sender, receiver) = std::sync::mpsc::channel();
        let available = crate::update::AvailableUpdate {
            version: semver::Version::new(0, 6, 3),
            tag: "v0.6.3".into(),
        };
        sender.send(Ok(Some(available.clone()))).unwrap();
        let mut settings = settings_state();
        settings.waiting_for_scheduled_update = true;
        settings.update_status = Some("Checking for updates…".into());
        let mut ui = TuiState {
            settings: Some(settings),
            scheduled_update_check: Some(receiver),
            ..TuiState::default()
        };

        poll_scheduled_update(&mut ui, &env);

        assert_eq!(ui.scheduled_available_update, Some(available.clone()));
        assert_eq!(
            ui.settings.as_ref().unwrap().available_update,
            Some(available)
        );
        assert!(!scheduled_update_prompt_visible(&ui, ViewKind::Idle));
        ui.settings = None;
        assert!(!scheduled_update_prompt_visible(&ui, ViewKind::Forge));
        assert!(scheduled_update_prompt_visible(&ui, ViewKind::Idle));
    }

    #[test]
    fn settings_reuses_an_in_flight_scheduled_check() {
        let root = tempdir().unwrap();
        let env = test_env(root.path().to_path_buf());
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut settings = settings_state();
        settings.row = 16;
        let mut ui = TuiState {
            settings: Some(settings),
            scheduled_update_check: Some(receiver),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Enter, KeyModifiers::NONE, &env).unwrap();

        let settings = ui.settings.as_ref().unwrap();
        assert!(settings.waiting_for_scheduled_update);
        assert!(settings.update_check.is_none());
        assert_eq!(
            settings.update_status.as_deref(),
            Some("Checking for updates…")
        );
    }

    #[test]
    fn settings_warn_when_enabled_notifications_are_unavailable() {
        let mut settings = settings_state();
        let warning = settings_lines_with_notification_reason(
            &settings,
            false,
            120,
            40,
            Some("notify-send not found on PATH"),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(warning.contains("enabled · ⚠ notify-send not found on PATH"));

        settings.draft.preferences.desktop_notifications = false;
        let disabled = settings_lines_with_notification_reason(
            &settings,
            false,
            120,
            40,
            Some("notify-send not found on PATH"),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(disabled.contains("Notifications          disabled"));
        assert!(!disabled.contains("notify-send"));
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
    fn settings_apply_without_backend_change_does_not_regenerate_queue() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut draft = Config::default();
        draft.profile.goals = vec!["mobility".into()];

        assert!(apply_settings(&env, &draft).unwrap().is_none());
        assert_eq!(
            config::load_or_default(&env.paths).unwrap().profile.goals,
            vec!["mobility"]
        );
    }

    #[test]
    fn settings_apply_with_backend_change_regenerates_queue() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut previous = Config::default();
        previous.recommender.backend = RecommenderBackend::Codex;
        config::save(&env.paths, &previous).unwrap();

        let receiver = apply_settings(&env, &Config::default()).unwrap();
        assert!(receiver
            .expect("backend changes should regenerate future forges")
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_ok());
    }

    #[test]
    fn settings_save_without_backend_change_reports_plain_success() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root);
        let mut settings = settings_state();
        settings.draft.profile.goals = vec!["mobility".into()];
        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };

        handle_settings_key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL, &env).unwrap();

        assert!(ui.settings.is_none());
        assert!(ui.queue_regeneration.is_none());
        assert!(!ui.settings_regeneration);
        assert_eq!(ui.status_message.as_deref(), Some("Settings saved."));
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
    fn idle_dashboard_is_compact_and_focuses_forge() {
        let backend = BackendView {
            label: RecommenderBackend::OpenaiEnv.label().into(),
            unavailable: false,
            config_file: "/tmp/config.toml".into(),
        };
        let activity = ForgeActivitySummary {
            today: ForgeActivityTotals { forges: 2, reps: 9 },
            week: ForgeActivityTotals {
                forges: 35,
                reps: 210,
            },
        };
        let nutrition = NutritionTotals {
            calories: 2021.0,
            protein_g: 148.0,
            ..NutritionTotals::default()
        };
        let usage = RecommenderTokenUsageByProvider {
            openai: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 10_000,
                    output_tokens: 15_000,
                },
                week: TokenUsageTotals {
                    input_tokens: 30_000,
                    output_tokens: 45_000,
                },
            },
            ..RecommenderTokenUsageByProvider::default()
        };
        let lines = idle_lines(
            WaitingDashboard {
                backend: &backend,
                activity: &activity,
                nutrition: &nutrition,
                nutrition_average: None,
                weight_progress: Some(WeightProgress {
                    starting_kg: 80.0,
                    current_kg: 78.0,
                }),
                unit_system: UnitSystem::Metric,
                token_usage: &usage,
                focused: WaitingSection::Forge,
                status_message: None,
                queue_loader_frame: None,
                queue_feedback: None,
                forge_now_feedback: None,
                area_width: 80,
            },
            false,
        );
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(lines.len(), 12);
        assert!(text.contains("> Forge   2 today · 9 reps · 35 this week"));
        assert!(text.contains("  Fuel    2021 kcal · 148P · ↓ 2.0 kg"));
        assert!(text.contains("  API     $0.02 today · $0.06 week"));
        assert!(text.contains("OpenAI · ready"));
        assert!(text.contains("↑↓ Select   Enter Open"));
        assert!(text.contains("[s] Settings   [q] Quit"));
        assert!(!text.contains("Latest forges"));
        assert!(!text.contains("Input"));
    }

    #[test]
    fn summaries_compact_before_wrapping_and_fuel_falls_back_to_average() {
        let average = LoggedDayNutritionAverage {
            totals: NutritionTotals {
                calories: 1999.0,
                protein_g: 147.6,
                ..NutritionTotals::default()
            },
            logged_days: 3,
        };
        assert_eq!(
            forge_summary_text(
                &ForgeActivitySummary {
                    today: ForgeActivityTotals { forges: 2, reps: 9 },
                    week: ForgeActivityTotals {
                        forges: 35,
                        reps: 210
                    },
                },
                34,
            ),
            "2 today · 35 week"
        );
        assert_eq!(
            fuel_summary_text(
                &NutritionTotals::default(),
                Some(&average),
                Some(WeightProgress {
                    starting_kg: 80.0,
                    current_kg: 78.0
                }),
                UnitSystem::Metric,
                30,
            ),
            "1999 kcal · 148P"
        );
    }

    #[test]
    fn detail_views_group_full_metrics_vertically() {
        let activity = ForgeActivitySummary {
            today: ForgeActivityTotals { forges: 2, reps: 9 },
            week: ForgeActivityTotals {
                forges: 35,
                reps: 210,
            },
        };
        let forge = forge_detail_lines(&activity, None, None, None, None, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(forge.contains("Today\n  2 forges\n  9 reps"));
        assert!(forge.contains("[l] Latest forges [n] Next forges"));

        let average = LoggedDayNutritionAverage {
            totals: NutritionTotals {
                calories: 2021.0,
                protein_g: 148.0,
                carbohydrates_g: 169.0,
                fat_g: 85.0,
                sugar_g: 46.0,
                ..NutritionTotals::default()
            },
            logged_days: 3,
        };
        let fuel = fuel_detail_lines(
            &NutritionTotals::default(),
            Some(&average),
            Some(WeightProgress {
                starting_kg: 80.0,
                current_kg: 78.0,
            }),
            UnitSystem::Metric,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(fuel.contains("Today\n  0 kcal\n  Protein    0 g"));
        assert!(fuel.contains("3-day average\n  2021 kcal\n  Protein    148 g"));
        assert!(fuel.contains("Weight\n↓ 2.0 kg"));
        assert!(fuel.contains("[a] Add fuel"));
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
            nutrition_average: None,
            weight_progress: None,
            unit_system: UnitSystem::Metric,
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
        let text = view_lines(&view, &TuiState::default(), 80)
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
        assert!(text.contains("[Esc] Back   [q] Quit"));
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
        assert!(text.contains("[Esc] Back   [q] Quit"));
    }

    #[test]
    fn waiting_navigation_wraps_and_preserves_section_focus() {
        let mut ui = TuiState::default();
        assert_eq!(ui.waiting_section, WaitingSection::Forge);

        assert!(handle_waiting_navigation(
            &mut ui,
            KeyCode::Up,
            ViewKind::Idle
        ));
        assert_eq!(ui.waiting_section, WaitingSection::Api);
        assert!(handle_waiting_navigation(
            &mut ui,
            KeyCode::Down,
            ViewKind::Idle
        ));
        assert_eq!(ui.waiting_section, WaitingSection::Forge);
        assert!(handle_waiting_navigation(
            &mut ui,
            KeyCode::Down,
            ViewKind::Idle
        ));
        assert_eq!(ui.waiting_section, WaitingSection::Fuel);
        assert!(handle_waiting_navigation(
            &mut ui,
            KeyCode::Enter,
            ViewKind::Idle
        ));
        assert_eq!(ui.waiting_page, WaitingPage::Fuel);
        assert!(handle_waiting_navigation(
            &mut ui,
            KeyCode::Esc,
            ViewKind::Idle
        ));
        assert_eq!(ui.waiting_page, WaitingPage::Dashboard);
        assert_eq!(ui.waiting_section, WaitingSection::Fuel);

        ui.waiting_section = WaitingSection::Forge;
        handle_waiting_navigation(&mut ui, KeyCode::Enter, ViewKind::Cooldown);
        handle_waiting_navigation(&mut ui, KeyCode::Char('l'), ViewKind::Cooldown);
        assert_eq!(ui.waiting_page, WaitingPage::History);
        handle_waiting_navigation(&mut ui, KeyCode::Esc, ViewKind::Cooldown);
        assert_eq!(ui.waiting_page, WaitingPage::Forge);

        assert!(forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Idle,
            WaitingPage::Forge
        ));
        assert!(!forge_now_requested(
            KeyCode::Char('f'),
            ViewKind::Idle,
            WaitingPage::History
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
        assert!(text.contains("[Esc] Back   [q] Quit"));
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
        assert!(
            no_safe.contains("Recommender returned no compatible forges. Keeping current list.")
        );
    }

    #[test]
    fn next_forge_lines_show_manual_forge_feedback() {
        let text = next_forge_lines(
            &[rec()],
            false,
            None,
            None,
            Some("Queued forges are still cooling down. Keeping current list."),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("Queued forges are still cooling down. Keeping current list."));
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
            Some("Queued forges are still cooling down. Keeping current list."),
        ));
        assert!(no_safe.contains("Queued forges are still cooling down."));
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
            WaitingPage::Next
        ));
        assert!(!regenerate_queue_requested(
            KeyCode::Char('r'),
            ViewKind::Idle,
            WaitingPage::Forge
        ));
        assert!(!regenerate_queue_requested(
            KeyCode::Char('r'),
            ViewKind::Forge,
            WaitingPage::Next
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
    fn api_cost_combines_input_and_output_before_rounding() {
        assert_eq!(api_cost(TokenUsageTotals::default()), "$0.00");
        assert_eq!(
            api_cost(TokenUsageTotals {
                input_tokens: 56_200,
                output_tokens: 21_700,
            }),
            "$0.04"
        );
        assert_eq!(
            api_cost(TokenUsageTotals {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            }),
            "$1.40"
        );
    }

    #[test]
    fn api_detail_keeps_tokens_and_provider_specific_cost() {
        let usage = RecommenderTokenUsageByProvider {
            codex: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 56_200,
                    output_tokens: 21_700,
                },
                week: TokenUsageTotals::default(),
            },
            openai: RecommenderTokenUsageSummary {
                today: TokenUsageTotals {
                    input_tokens: 56_200,
                    output_tokens: 21_700,
                },
                week: TokenUsageTotals::default(),
            },
        };
        let openai = BackendView {
            label: RecommenderBackend::OpenaiEnv.label().into(),
            unavailable: false,
            config_file: String::new(),
        };
        let text = api_detail_lines(&openai, &usage, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Input      56.2k"));
        assert!(text.contains("Output     21.7k"));
        assert!(text.contains("Cost       $0.04"));

        let codex = BackendView {
            label: "Codex".into(),
            ..openai
        };
        let text = api_detail_lines(&codex, &usage, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Cost       unavailable"));
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
        let nutrition = NutritionTotals::default();
        let usage = RecommenderTokenUsageByProvider::default();
        let recommendation = rec();
        let ui = TuiState {
            demo: true,
            ..TuiState::default()
        };
        let dashboard = || WaitingDashboard {
            backend: &backend,
            activity: &activity,
            nutrition: &nutrition,
            nutrition_average: None,
            weight_progress: None,
            unit_system: UnitSystem::Metric,
            token_usage: &usage,
            focused: WaitingSection::Forge,
            status_message: None,
            queue_loader_frame: None,
            queue_feedback: None,
            forge_now_feedback: None,
            area_width: 80,
        };
        let screens = vec![
            idle_lines(dashboard(), true),
            cooldown_lines(dashboard(), true),
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
    fn recommender_status_is_compact_and_emphasizes_failures() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/config.toml".to_string(),
        };
        let line = recommender_status_line(&backend);
        assert_eq!(line.to_string(), "Codex · ready");
        assert_eq!(line.spans[0].style, muted());

        let unavailable = recommender_status_line(&BackendView {
            unavailable: true,
            ..backend
        });
        assert_eq!(unavailable.to_string(), "⚠ Codex unavailable");
        assert_eq!(unavailable.spans[0].style, accent_bold());
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
    fn unavailable_backend_is_prominent_without_configuration_source() {
        let backend = BackendView {
            label: "OpenAI (environment)".to_string(),
            unavailable: true,
            config_file: "/tmp/svarog/config.toml".to_string(),
        };
        let activity = ForgeActivitySummary::default();
        let nutrition = NutritionTotals::default();
        let usage = RecommenderTokenUsageByProvider::default();
        let text = idle_lines(
            WaitingDashboard {
                backend: &backend,
                activity: &activity,
                nutrition: &nutrition,
                nutrition_average: None,
                weight_progress: None,
                unit_system: UnitSystem::Metric,
                token_usage: &usage,
                focused: WaitingSection::Forge,
                status_message: None,
                queue_loader_frame: None,
                queue_feedback: None,
                forge_now_feedback: None,
                area_width: 80,
            },
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(text.contains("⚠ OpenAI unavailable"));
        assert!(!text.contains("(environment)"));
        assert!(!text.contains("/tmp/svarog/config.toml"));
    }

    #[test]
    fn api_summaries_follow_the_active_backend() {
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
        assert_eq!(
            api_summary_text(&openai, &usage, 80),
            "$0.03 today · $0.03 week"
        );

        let local = BackendView {
            label: "Local".into(),
            unavailable: false,
            config_file: "/tmp/svarog/config.toml".into(),
        };
        assert_eq!(api_summary_text(&local, &usage, 80), "No remote usage");

        let codex = BackendView {
            label: "Codex".into(),
            ..local
        };
        assert_eq!(
            api_summary_text(&codex, &usage, 80),
            "12.4k in · 320 out today"
        );
    }

    #[test]
    fn idle_lines_show_recommender_status_message() {
        let backend = BackendView {
            label: "Codex".to_string(),
            unavailable: false,
            config_file: "/tmp/svarog/config.toml".to_string(),
        };
        let activity = ForgeActivitySummary::default();
        let nutrition = NutritionTotals::default();
        let usage = RecommenderTokenUsageByProvider::default();
        let text = idle_lines(
            WaitingDashboard {
                backend: &backend,
                activity: &activity,
                nutrition: &nutrition,
                nutrition_average: None,
                weight_progress: None,
                unit_system: UnitSystem::Metric,
                token_usage: &usage,
                focused: WaitingSection::Forge,
                status_message: Some("Could not update recommender. Edit: /tmp/svarog/config.toml"),
                queue_loader_frame: None,
                queue_feedback: None,
                forge_now_feedback: None,
                area_width: 80,
            },
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
        assert!(!text.contains("Target"));
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
    fn quit_hint_remains_visible_with_tmux_help() {
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
            nutrition_average: None,
            weight_progress: None,
            unit_system: UnitSystem::Metric,
            token_usage: RecommenderTokenUsageByProvider::default(),
            history: Vec::new(),
            next_forges: Vec::new(),
        };

        for in_tmux in [false, true] {
            let lines = screen_lines(&view, &TuiState::default(), in_tmux, 80);
            assert!(lines
                .iter()
                .any(|line| line.to_string().contains("[q] Quit")));
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
    fn add_fuel_shortcut_is_available_on_dashboard_and_fuel_detail() {
        assert!(add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Idle,
            WaitingPage::Dashboard
        ));
        assert!(add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Cooldown,
            WaitingPage::Fuel
        ));
        assert!(!add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Forge,
            WaitingPage::Dashboard
        ));
        assert!(!add_fuel_requested(
            KeyCode::Char('a'),
            ViewKind::Idle,
            WaitingPage::History
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
        assert!(rendered.contains("[enter] Log that fuel · 16/2000"));
        assert!(rendered.contains("200 ml  [+/-] 200 ml"));
        assert!(rendered.contains("Today’s fuel\n120 kcal · P 3.0g · C 14.0g · F 5.0g · S 8.0g"));
        assert!(rendered.contains("Recent fuel"));
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
    fn add_fuel_wraps_and_preserves_complete_error_feedback() {
        let mut state = add_fuel_state_for_test();
        let feedback = "Could not parse fuel: parsing meal or drink with OpenAI: OpenAI Responses API returned 429 Too Many Requests: You exceeded your current quota; check your plan and billing details.";
        state.feedback = Some(feedback.into());

        let lines = add_fuel_lines(&state, false, 40, 40);
        assert!(lines
            .iter()
            .any(|line| { line.to_string() == feedback.chars().take(40).collect::<String>() }));
        let rendered_without_line_breaks = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered_without_line_breaks.contains(feedback));
    }

    #[test]
    fn add_fuel_water_total_uses_standard_text_color() {
        let state = add_fuel_state_for_test();
        let lines = add_fuel_lines(&state, false, 120, 40);
        let water_line = lines
            .iter()
            .find(|line| line.to_string().contains("0 ml  [+/-] 200 ml"))
            .unwrap();
        assert_eq!(water_line.spans[1].style.fg, Some(colors::TEXT));
    }

    #[test]
    fn weight_trend_uses_arrows_and_selected_units() {
        let lost = weight_trend_text(
            Some(WeightProgress {
                starting_kg: 80.0,
                current_kg: 77.0,
            }),
            UnitSystem::Metric,
        )
        .unwrap();
        assert_eq!(lost, "↓ 3.0 kg");

        let gained = weight_trend_text(
            Some(WeightProgress {
                starting_kg: 70.0,
                current_kg: 71.0,
            }),
            UnitSystem::Imperial,
        )
        .unwrap();
        assert_eq!(gained, "↑ 2.2 lb");

        assert!(weight_trend_text(None, UnitSystem::Metric).is_none());
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
                120,
                40,
            ));
        }

        assert_eq!(ui.add_fuel.unwrap().water.milliliters, 200.0);
    }

    #[test]
    fn add_fuel_accepts_and_renders_a_multiline_whole_day_paste() {
        let pasted = "August 16th:\r\n11 am - single espresso with 80 ml of 3.2% milk\r\n1 pm - single espresso with 80 ml of 3.2% milk\r\n1 pm - 200 g of mashed potatoes, 100 g of mashed potatoes, 50 g of peas, 20 g of butter\r\n4 pm - single espresso with 80 ml of 3.2% milk\r\n5 pm - 200 g of mashed potatoes, 100 g of mashed potatoes, 50 g of peas, 20 g of butter\r\n9 pm - single espresso with 80 ml of 3.2% milk\r\n9 pm - 5 hard boiled eggs";
        let mut ui = TuiState {
            add_fuel: Some(add_fuel_state_for_test()),
            ..TuiState::default()
        };

        handle_add_fuel_paste(&mut ui, pasted, 100, 40);

        let state = ui.add_fuel.as_ref().unwrap();
        assert_eq!(state.input.matches('\n').count(), 7);
        assert!(!state.input.contains('\r'));
        let rendered = meal_editor_lines(state, 100)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("August 16th:"));
        assert!(rendered.contains("11 am - single espresso"));
        assert!(rendered.contains("1 pm - 200 g of mashed potatoes"));
        assert!(rendered.contains("5 pm - 200 g of mashed potatoes"));
        assert!(rendered.contains("9 pm - 5 hard boiled eggs"));
    }

    #[test]
    fn add_fuel_enter_submits_with_or_without_modifiers() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root.clone());
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let mut state = add_fuel_state_for_test();
        state.backend = RecommenderBackend::Local;
        state.input = "meal".into();
        state.cursor = 4;
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };

        for modifier in [
            KeyModifiers::NONE,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
        ] {
            assert!(!handle_add_fuel_key(
                &mut ui,
                KeyCode::Enter,
                modifier,
                &env,
                &store,
                80,
                24,
            ));
            assert!(ui
                .add_fuel
                .as_ref()
                .unwrap()
                .feedback
                .as_deref()
                .unwrap()
                .contains("needs Codex or an OpenAI backend"));
            assert_eq!(ui.add_fuel.as_ref().unwrap().input, "meal");
        }
    }

    #[test]
    fn add_fuel_paste_enforces_limit_and_is_ignored_outside_meal_editing() {
        let mut state = add_fuel_state_for_test();
        state.input = "a".repeat(fuel::MAX_FUEL_INPUT_CHARS - 1);
        state.cursor = state.input.chars().count();
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };
        handle_add_fuel_paste(&mut ui, "bc", 80, 24);
        let state = ui.add_fuel.as_ref().unwrap();
        assert_eq!(state.input.chars().count(), fuel::MAX_FUEL_INPUT_CHARS);
        assert!(state.feedback.as_deref().unwrap().contains("limited"));

        ui.add_fuel.as_mut().unwrap().focus = AddFuelFocus::Water;
        handle_add_fuel_paste(&mut ui, "ignored", 80, 24);
        assert!(!ui.add_fuel.as_ref().unwrap().input.contains("ignored"));
    }

    #[test]
    fn multiline_editor_moves_vertically_and_keeps_recent_selection_visible() {
        let (unicode_rows, _) = editor_layout("a界b", 3);
        assert_eq!(unicode_rows, vec!["a界", "b"]);

        let input = "first line\nsecond line\nthird line";
        let second_line = input.find("second").unwrap();
        assert_eq!(
            move_cursor_vertical(input, second_line, 80, false),
            input.find("first").unwrap()
        );
        assert_eq!(
            move_cursor_vertical(input, second_line, 80, true),
            input.find("third").unwrap()
        );

        let root = tempdir().unwrap().keep();
        let env = test_env(root.clone());
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let mut state = add_fuel_state_for_test();
        state.focus = AddFuelFocus::Recent;
        state.recent = (0..5)
            .map(|index| FuelEntry {
                id: i64::from(index),
                raw_text: format!("meal {index}"),
                parsed: FuelParseResult {
                    items: vec![FuelItem {
                        name: format!("item {index}"),
                        quantity: None,
                        unit: None,
                        nutrition: NutritionTotals::default(),
                        assumptions: Vec::new(),
                    }],
                },
                totals: NutritionTotals::default(),
                provider: "codex".into(),
                model: fuel::FUEL_MODEL.into(),
                created_at: chrono::Utc::now() - ChronoDuration::minutes(i64::from(index)),
            })
            .collect();
        state.scroll = focus_scroll_hint(&state, 50);
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };

        for index in 0..5 {
            if index > 0 {
                handle_add_fuel_key(
                    &mut ui,
                    KeyCode::Down,
                    KeyModifiers::NONE,
                    &env,
                    &store,
                    50,
                    7,
                );
            }
            let rendered = add_fuel_lines(ui.add_fuel.as_ref().unwrap(), false, 50, 7)
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(rendered.contains(&format!("item {index}")));
        }
    }

    #[test]
    fn bracketed_paste_keeps_settings_text_entry_working() {
        let mut settings = settings_state();
        settings.editing = true;
        settings.row = 14;
        let mut ui = TuiState {
            settings: Some(settings),
            ..TuiState::default()
        };

        handle_settings_paste(&mut ui, "posture and stretching");

        assert_eq!(
            ui.settings.as_ref().unwrap().edit_value.as_str(),
            "posture and stretching"
        );
    }

    #[test]
    fn recent_fuel_uses_consumption_time_and_event_items() {
        let parsed = FuelParseResult {
            items: vec![
                FuelItem {
                    name: "eggs".into(),
                    quantity: Some(400.0),
                    unit: Some("g".into()),
                    nutrition: NutritionTotals::default(),
                    assumptions: Vec::new(),
                },
                FuelItem {
                    name: "peas".into(),
                    quantity: Some(200.0),
                    unit: Some("g".into()),
                    nutrition: NutritionTotals::default(),
                    assumptions: Vec::new(),
                },
            ],
        };
        let consumed_at = (Local::now() - ChronoDuration::days(1)).with_timezone(&chrono::Utc);
        let entry = FuelEntry {
            id: 1,
            raw_text: "the complete original whole-day description".into(),
            totals: parsed.totals(),
            parsed,
            provider: "codex".into(),
            model: fuel::FUEL_MODEL.into(),
            created_at: consumed_at,
        };

        let label = recent_fuel_label(&entry, Local::now().date_naive());
        assert!(label.starts_with("Yesterday "));
        assert!(label.contains("400 g eggs, 200 g peas"));
        assert!(!label.contains("complete original"));
    }

    #[test]
    fn reviewed_timeline_saves_equal_time_repeated_foods_and_refreshes_today() {
        let root = tempdir().unwrap().keep();
        let env = test_env(root.clone());
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let parsed = FuelParseResult {
            items: vec![FuelItem {
                name: "milk".into(),
                quantity: Some(1.0),
                unit: Some("serving".into()),
                nutrition: NutritionTotals {
                    calories: 100.0,
                    protein_g: 5.0,
                    ..NutritionTotals::default()
                },
                assumptions: Vec::new(),
            }],
        };
        let today = Local::now().date_naive();
        let at = |hour| {
            Local
                .from_local_datetime(&today.and_hms_opt(hour, 0, 0).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let mut state = add_fuel_state_for_test();
        state.input = "milk in coffee at 10 and milk in a shake at 10".into();
        state.cursor = state.input.chars().count();
        state.parsed = Some(FuelParseOutcome {
            events: vec![
                TimedFuelEvent {
                    consumed_at: at(10),
                    source_text: "milk in coffee".into(),
                    parsed: parsed.clone(),
                },
                TimedFuelEvent {
                    consumed_at: at(10),
                    source_text: "milk in a shake".into(),
                    parsed,
                },
            ],
            inferred_yesterday: false,
            provider: "codex",
            model: fuel::FUEL_MODEL,
        });
        let mut ui = TuiState {
            add_fuel: Some(state),
            ..TuiState::default()
        };

        let review = fuel_review_lines(
            ui.add_fuel.as_ref().unwrap().parsed.as_ref().unwrap(),
            false,
            None,
            120,
            40,
            0,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!(review.matches("Today · 10:00").count(), 2);

        assert!(!handle_add_fuel_key(
            &mut ui,
            KeyCode::Enter,
            KeyModifiers::NONE,
            &env,
            &store,
            120,
            40,
        ));

        let state = ui.add_fuel.unwrap();
        assert!(state.input.is_empty());
        assert_eq!(state.recent.len(), 2);
        assert_eq!(state.nutrition.calories, 200.0);
        assert_eq!(state.feedback.as_deref(), Some("✓ 2 meals saved"));
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

        let outcome = FuelParseOutcome {
            events: vec![TimedFuelEvent {
                consumed_at: chrono::Utc::now(),
                source_text: "meal".into(),
                parsed,
            }],
            inferred_yesterday: false,
            provider: "codex",
            model: fuel::FUEL_MODEL,
        };
        let first = fuel_review_lines(&outcome, false, None, 24, 8, 0);
        let last = fuel_review_lines(&outcome, false, None, 24, 8, usize::MAX);
        assert!(first
            .iter()
            .map(Line::to_string)
            .collect::<String>()
            .contains("[enter] Save 1 meal"));
        assert!(last
            .iter()
            .map(Line::to_string)
            .collect::<String>()
            .contains("[enter] Save 1 meal"));
        assert_ne!(first[0].to_string(), last[0].to_string());
        assert!(first.iter().all(|line| line.width() <= 24));
        assert!(last.iter().all(|line| line.width() <= 24));

        let inferred = FuelParseOutcome {
            inferred_yesterday: true,
            ..outcome
        };
        let text = fuel_review_lines(&inferred, false, None, 80, 100, 0)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Date inferred as yesterday"));
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
