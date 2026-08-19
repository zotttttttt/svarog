use crate::config::{load_or_default, Config, RuntimeEnv};
use crate::engine;
use crate::models::{Agent, AppStateKind, CodexHookEvent, IncomingEvent, Recommendation};
use crate::notifications;
use crate::recommender;
use crate::secrets;
use crate::storage::Store;
use anyhow::{bail, Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

struct AppState {
    env: RuntimeEnv,
    accept_codex: bool,
    event_lock: Mutex<()>,
}

pub struct Collector {
    #[cfg(test)]
    addr: std::net::SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<()>>,
    _lock: TuiLock,
}

#[derive(Debug)]
struct TuiLock {
    file: File,
}

static REFILL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct RefillGuard;

impl Drop for RefillGuard {
    fn drop(&mut self) {
        REFILL_IN_PROGRESS.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRegenerationOutcome {
    pub source: recommender::QueueGenerationSource,
    pub notice: Option<String>,
    pub llm_count: usize,
    pub local_count: usize,
}

pub const NO_SAFE_FORGES_ERROR: &str = "no safe forges are available right now";

pub type QueueRegenerationResult = std::result::Result<QueueRegenerationOutcome, String>;

pub enum QueueRegenerationStart {
    Started(Receiver<QueueRegenerationResult>),
    Busy,
}

#[derive(Debug)]
pub enum ForgeNowResult {
    Started,
    DailyForgeCeilingReached { completed: u32, limit: u32 },
    NoQueued,
    CoolingDown,
}

#[derive(Serialize)]
pub struct EventResponse {
    pub recommended: bool,
    pub recommendation: Option<Recommendation>,
    pub notice: Option<String>,
}

pub async fn run() -> Result<()> {
    let env = RuntimeEnv::load()?;
    let _openai_key_cache_guard = secrets::openai_key_cache_guard(&env.paths);
    env.paths.ensure()?;
    let addr = env.daemon_addr;
    refill_queue_best_effort(&env);
    let app = router(env, false);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding daemon to {addr}"))?;
    axum::serve(listener, app).await.context("running daemon")
}

impl Collector {
    pub async fn start(env: &RuntimeEnv) -> Result<Self> {
        env.paths.ensure()?;
        let tui_lock = TuiLock::acquire(env)?;
        let addr = env.daemon_addr;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| {
                format!(
                    "starting Svarog collector on {addr}; if an older Svarog daemon is still running, stop it once and retry"
                )
            })?;
        #[cfg(test)]
        let bound_addr = listener.local_addr().context("reading collector address")?;
        let app = router(env.clone(), true);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .context("running Svarog collector")
        });
        refill_queue_best_effort(env);
        Ok(Self {
            #[cfg(test)]
            addr: bound_addr,
            shutdown: Some(shutdown),
            task,
            _lock: tui_lock,
        })
    }

    #[cfg(test)]
    fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.await.context("joining Svarog collector")?
    }
}

pub fn ensure_tui_available(env: &RuntimeEnv) -> Result<()> {
    env.paths.ensure()?;
    drop(TuiLock::acquire(env)?);
    Ok(())
}

impl TuiLock {
    fn acquire(env: &RuntimeEnv) -> Result<Self> {
        let path = env.paths.data_dir.join("tui.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            let pid = pid.trim();
            if pid.is_empty() {
                bail!("Svarog TUI is already running");
            }
            bail!("Svarog TUI is already running (PID {pid})");
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { file })
    }
}

impl Drop for TuiLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn router(env: RuntimeEnv, accept_codex: bool) -> Router {
    let state = AppState {
        env,
        accept_codex,
        event_lock: Mutex::new(()),
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/events", post(handle_event))
        .route("/hooks/codex", post(handle_codex_hook))
        .with_state(Arc::new(state))
}

async fn handle_event(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<IncomingEvent>,
) -> Result<Json<EventResponse>, (StatusCode, String)> {
    let _guard = state.event_lock.lock().await;
    process_event(&state.env, payload)
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

async fn handle_codex_hook(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CodexHookEvent>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !state.accept_codex {
        return Ok(StatusCode::NO_CONTENT);
    }
    let _guard = state.event_lock.lock().await;
    process_codex_hook(&state.env, payload)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub fn process_codex_hook(env: &RuntimeEnv, payload: CodexHookEvent) -> Result<()> {
    let store = Store::open(&env.paths.database_file)?;
    match payload.hook_event_name.as_str() {
        "SessionStart" => {
            store.record_codex_session(&payload)?;
        }
        "UserPromptSubmit" => {
            if store.record_codex_prompt(&payload)? {
                let event = IncomingEvent {
                    agent: Agent::Codex,
                    event: "user_prompt_submit".to_string(),
                    expected_duration_sec: None,
                    duration_sec: None,
                    project: payload.project(),
                };
                process_event(env, event)?;
            }
        }
        "Stop" => store.record_codex_stop(&payload)?,
        "SessionEnd" => store.record_codex_session_end(&payload)?,
        _ => {}
    }
    Ok(())
}

pub fn process_event(env: &RuntimeEnv, payload: IncomingEvent) -> Result<EventResponse> {
    process_event_with_notifier(env, payload, &notifications::notify)
}

fn process_event_with_notifier(
    env: &RuntimeEnv,
    payload: IncomingEvent,
    notifier: &dyn Fn(bool, &str, &str) -> bool,
) -> Result<EventResponse> {
    let paths = &env.paths;
    let config = load_or_default(paths)?;
    let store = Store::open(&paths.database_file)?;
    ensure_exercise_pool(&store, &config)?;
    let event = payload.into_event_with_default(config.preferences.default_expected_duration_sec);
    store.insert_event(&event)?;
    if store.fatigue_suppression_active()? {
        return Ok(EventResponse {
            recommended: false,
            recommendation: None,
            notice: None,
        });
    }

    let state = store.state()?;
    let open_recommendation = store.latest_open_recommendation()?;
    if matches!(
        state.kind,
        AppStateKind::Recommendation | AppStateKind::Active
    ) || open_recommendation.is_some()
    {
        if event.agent == Agent::Codex && event.event == "user_prompt_submit" {
            if let Some(rec) = open_recommendation.as_ref() {
                notify_recommendation(notifier, config.preferences.desktop_notifications, rec);
            }
        }
        return Ok(EventResponse {
            recommended: false,
            recommendation: None,
            notice: None,
        });
    }
    if !crate::engine::opportunity_allows(&store, &config)? {
        return Ok(EventResponse {
            recommended: false,
            recommendation: None,
            notice: None,
        });
    }

    let notice = None;
    let recommendation =
        match store.promote_next_queued_recommendation(event.agent, event.project.as_deref())? {
            Some(rec) => Some(rec),
            None => match engine::recommend(&store, &config, &event)? {
                Some(mut rec) => {
                    let id = store.insert_recommendation(&rec)?;
                    rec.id = Some(id);
                    Some(rec)
                }
                None => None,
            },
        };

    if let Some(rec) = recommendation.as_ref() {
        notify_recommendation(notifier, config.preferences.desktop_notifications, rec);
    }
    if store.queued_recommendation_count()? == 0 {
        refill_queue_best_effort(env);
    }
    Ok(EventResponse {
        recommended: recommendation.is_some(),
        recommendation,
        notice,
    })
}

fn notify_recommendation(
    notifier: &dyn Fn(bool, &str, &str) -> bool,
    enabled: bool,
    recommendation: &Recommendation,
) {
    notifier(
        enabled,
        "Svarog",
        &format!("{} {}", recommendation.reps, recommendation.display_name()),
    );
}

pub fn refill_queue_best_effort(env: &RuntimeEnv) {
    if !begin_queue_job(&REFILL_IN_PROGRESS) {
        return;
    }
    let env = env.clone();
    std::thread::spawn(move || {
        let _guard = RefillGuard;
        let Ok(config) = load_or_default(&env.paths) else {
            return;
        };
        let Ok(store) = Store::open(&env.paths.database_file) else {
            return;
        };
        let _ = ensure_exercise_pool(&store, &config);
        let _ = recommender::fill_recommendation_queue(&store, &config, &env.paths);
    });
}

pub fn regenerate_queue_best_effort(env: &RuntimeEnv) {
    if !begin_queue_job(&REFILL_IN_PROGRESS) {
        return;
    }
    let env = env.clone();
    std::thread::spawn(move || {
        let _guard = RefillGuard;
        let _ = regenerate_queue_now(&env);
    });
}

pub fn regenerate_queue_after_settings(env: &RuntimeEnv) -> Receiver<QueueRegenerationResult> {
    let (sender, receiver) = mpsc::channel();
    let env = env.clone();
    std::thread::spawn(move || {
        for _ in 0..300 {
            if begin_queue_job(&REFILL_IN_PROGRESS) {
                let _guard = RefillGuard;
                let result = regenerate_queue_now(&env).map_err(|err| err.to_string());
                let _ = sender.send(result);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = sender.send(Err(
            "timed out waiting to refresh future forges; the existing queue was kept".into(),
        ));
    });
    receiver
}

pub fn regenerate_queue(env: &RuntimeEnv) -> QueueRegenerationStart {
    if !begin_queue_job(&REFILL_IN_PROGRESS) {
        return QueueRegenerationStart::Busy;
    }
    let (sender, receiver) = mpsc::channel();
    let env = env.clone();
    std::thread::spawn(move || {
        let _guard = RefillGuard;
        let result = regenerate_queue_now(&env).map_err(|err| err.to_string());
        let _ = sender.send(result);
    });
    QueueRegenerationStart::Started(receiver)
}

pub fn forge_now(env: &RuntimeEnv) -> Result<ForgeNowResult> {
    let config = load_or_default(&env.paths)?;
    let store = Store::open(&env.paths.database_file)?;
    let completed = store.today_set_count()?;
    let limit = config.preferences.max_daily_sets;
    if completed >= limit {
        return Ok(ForgeNowResult::DailyForgeCeilingReached { completed, limit });
    }
    if store.queued_recommendation_count()? == 0 {
        return Ok(ForgeNowResult::NoQueued);
    }
    Ok(
        match store.promote_next_queued_recommendation_preserving_metadata()? {
            Some(_) => ForgeNowResult::Started,
            None => ForgeNowResult::CoolingDown,
        },
    )
}

fn begin_queue_job(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn regenerate_queue_now(env: &RuntimeEnv) -> Result<QueueRegenerationOutcome> {
    let config = load_or_default(&env.paths)?;
    let mut store = Store::open(&env.paths.database_file)?;
    ensure_exercise_pool(&store, &config)?;
    let generated = recommender::generate_recommendation_queue(
        &store,
        &config,
        &env.paths,
        recommender::QUEUE_TARGET,
    )?;
    if generated.recommendations.is_empty() {
        bail!(NO_SAFE_FORGES_ERROR);
    }
    store.replace_queued_recommendations(&generated.recommendations)?;
    Ok(QueueRegenerationOutcome {
        source: generated.source,
        notice: generated.notice,
        llm_count: generated.llm_count,
        local_count: generated.local_count,
    })
}

pub fn refresh_exercise_pool(env: &RuntimeEnv) -> Result<()> {
    let config = load_or_default(&env.paths)?;
    let store = Store::open(&env.paths.database_file)?;
    ensure_exercise_pool(&store, &config)
}

fn ensure_exercise_pool(store: &Store, config: &Config) -> Result<()> {
    if store.exercise_catalog_is_current()? {
        return Ok(());
    }
    let equipment =
        crate::exercise_catalog::locally_resolved_equipment(&config.profile.equipment_text);
    let movements = crate::exercise_catalog::movements_for_equipment(&equipment);
    store.save_exercise_filter(&crate::recommender::normalize_equipment(
        &config.profile.equipment_text,
    ))?;
    store.replace_movement_pool(&movements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths, Recommender, RecommenderBackend, RuntimeMode};
    use crate::models::{Agent, Movement, MovementSidedness, MovementStatus, SetStatus};
    use std::cell::RefCell;
    use tempfile::tempdir;

    fn test_env() -> RuntimeEnv {
        let root = tempdir().unwrap().keep();
        RuntimeEnv {
            mode: RuntimeMode::Dev,
            paths: Paths::from_root(root.join("svarog")),
            codex_home: root.join("codex"),
            daemon_addr: "127.0.0.1:18787".parse().unwrap(),
            dry_run: false,
        }
    }

    fn event() -> IncomingEvent {
        IncomingEvent {
            agent: Agent::Codex,
            event: "tool_start".into(),
            expected_duration_sec: Some(120),
            duration_sec: None,
            project: Some("svarog".into()),
        }
    }

    fn codex_prompt_event() -> IncomingEvent {
        IncomingEvent {
            event: "user_prompt_submit".into(),
            ..event()
        }
    }

    #[test]
    fn queue_jobs_are_serialized() {
        let flag = AtomicBool::new(false);

        assert!(begin_queue_job(&flag));
        assert!(!begin_queue_job(&flag));
        flag.store(false, Ordering::Release);
        assert!(begin_queue_job(&flag));
    }

    #[test]
    fn solo_policy_refresh_retires_partner_exercises_and_preserves_history() {
        let env = test_env();
        crate::config::save(&env.paths, &Config::default()).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        store
            .replace_movement_pool(&[Movement {
                id: "Seated_Biceps".into(),
                name: "Seated Biceps".into(),
                primary_muscle: "biceps".into(),
                muscles: vec!["biceps".into()],
                equipment: vec!["bodyweight".into()],
                base_reps: 4,
                estimated_seconds: 35,
                status: MovementStatus::Allowed,
                mobility: true,
                sidedness: MovementSidedness::Bilateral,
            }])
            .unwrap();
        store.save_exercise_filter(&Vec::<String>::new()).unwrap();
        let mut partner = Recommendation {
            id: None,
            movement_id: "Seated_Biceps".into(),
            movement_name: "Seated Biceps".into(),
            primary_muscle: "biceps".into(),
            muscles: vec!["biceps".into()],
            reps: 4,
            weight_kg: None,
            estimated_seconds: 35,
            agent: Agent::Codex,
            project: Some("svarog".into()),
            side: None,
            created_at: chrono::Utc::now(),
        };
        partner.id = Some(store.insert_recommendation(&partner).unwrap());
        store.record_set(&partner, SetStatus::Skipped).unwrap();
        store
            .mark_recommendation(partner.id.unwrap(), "active")
            .unwrap();
        assert!(!store.exercise_catalog_is_current().unwrap());
        drop(store);

        refresh_exercise_pool(&env).unwrap();

        let store = Store::open(&env.paths.database_file).unwrap();
        assert!(store.exercise_catalog_is_current().unwrap());
        assert!(store
            .movements()
            .unwrap()
            .iter()
            .all(|movement| movement.id != "Seated_Biceps"));
        assert!(store.latest_open_recommendation().unwrap().is_none());
        assert_eq!(store.recent_forge_history(10).unwrap().len(), 1);
    }

    #[test]
    fn regeneration_replaces_an_existing_queue() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let mut old = Recommendation {
            id: None,
            movement_id: "Dead_Bug".into(),
            movement_name: "old queued forge".into(),
            primary_muscle: "old".into(),
            muscles: vec!["old".into()],
            reps: 1,
            weight_kg: None,
            estimated_seconds: 1,
            agent: Agent::Custom,
            project: None,
            side: None,
            created_at: chrono::Utc::now(),
        };
        store.insert_queued_recommendation(&old).unwrap();
        old.movement_name = "another old forge".into();
        store.insert_queued_recommendation(&old).unwrap();
        drop(store);

        regenerate_queue_now(&env).unwrap();

        let store = Store::open(&env.paths.database_file).unwrap();
        let queued = store.queued_recommendations().unwrap();
        assert!(!queued.is_empty());
        assert!(queued.len() <= recommender::QUEUE_TARGET as usize);
        assert!(queued
            .iter()
            .all(|rec| !rec.movement_name.contains("old forge")));
    }

    #[test]
    fn failed_regeneration_preserves_the_existing_queue() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        config.recommender.codex.command = "missing-codex-for-test".into();
        config.recommender.local_fallback = false;
        crate::config::save(&env.paths, &config).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let old = Recommendation {
            id: None,
            movement_id: "Dead_Bug".into(),
            movement_name: "old queued forge".into(),
            primary_muscle: "old".into(),
            muscles: vec!["old".into()],
            reps: 1,
            weight_kg: None,
            estimated_seconds: 1,
            agent: Agent::Custom,
            project: None,
            side: None,
            created_at: chrono::Utc::now(),
        };
        store.insert_queued_recommendation(&old).unwrap();
        drop(store);

        assert!(regenerate_queue_now(&env).is_err());

        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(
            store.queued_recommendations().unwrap()[0].movement_name,
            "old queued forge"
        );
    }

    #[test]
    fn forge_now_promotes_the_first_safe_queued_recommendation() {
        let env = test_env();
        let store = Store::open(&env.paths.database_file).unwrap();
        let rec = Recommendation {
            id: None,
            movement_id: "manual-movement".into(),
            movement_name: "manual forge".into(),
            primary_muscle: "manual-muscle".into(),
            muscles: vec!["manual-muscle".into()],
            reps: 8,
            weight_kg: None,
            estimated_seconds: 30,
            agent: Agent::Claude,
            project: Some("manual-project".into()),
            side: None,
            created_at: chrono::Utc::now(),
        };
        store.insert_queued_recommendation(&rec).unwrap();
        drop(store);

        let result = forge_now(&env).unwrap();
        let ForgeNowResult::Started = result else {
            panic!("expected a queued forge to start");
        };

        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
        assert_eq!(
            store.state().unwrap().kind,
            crate::models::AppStateKind::Recommendation
        );
        let promoted = store.latest_open_recommendation().unwrap().unwrap();
        assert_eq!(promoted.movement_name, "manual forge");
        assert_eq!(promoted.agent, Agent::Claude);
        assert_eq!(promoted.project.as_deref(), Some("manual-project"));
    }

    #[test]
    fn forge_now_reports_an_empty_queue_without_starting_a_forge() {
        let env = test_env();

        assert!(matches!(forge_now(&env).unwrap(), ForgeNowResult::NoQueued));
    }

    #[test]
    fn forge_now_reports_when_all_queued_forges_are_cooling_down() {
        let env = test_env();
        crate::config::save(&env.paths, &Config::default()).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let mut completed = Recommendation {
            id: None,
            movement_id: "cooling-movement".into(),
            movement_name: "cooling forge".into(),
            primary_muscle: "cooling-muscle".into(),
            muscles: vec!["cooling-muscle".into()],
            reps: 8,
            weight_kg: None,
            estimated_seconds: 30,
            agent: Agent::Codex,
            project: None,
            side: None,
            created_at: chrono::Utc::now(),
        };
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store.record_set(&completed, SetStatus::Done).unwrap();
        let mut queued = completed.clone();
        queued.id = None;
        store.insert_queued_recommendation(&queued).unwrap();
        drop(store);

        assert!(matches!(
            forge_now(&env).unwrap(),
            ForgeNowResult::CoolingDown
        ));
    }

    #[test]
    fn forge_now_respects_the_daily_forge_ceiling_not_repetitions() {
        let env = test_env();
        crate::config::save(&env.paths, &Config::default()).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let mut completed = Recommendation {
            id: None,
            movement_id: "completed-movement".into(),
            movement_name: "completed forge".into(),
            primary_muscle: "completed-muscle".into(),
            muscles: vec!["completed-muscle".into()],
            reps: 1,
            weight_kg: None,
            estimated_seconds: 30,
            agent: Agent::Codex,
            project: None,
            side: None,
            created_at: chrono::Utc::now(),
        };
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        for _ in 0..100 {
            store
                .record_set_with_reps(&completed, SetStatus::Done, 1)
                .unwrap();
        }
        let mut queued = completed.clone();
        queued.id = None;
        queued.movement_id = "queued-movement".into();
        queued.movement_name = "queued forge".into();
        queued.primary_muscle = "queued-muscle".into();
        queued.muscles = vec!["queued-muscle".into()];
        store.insert_queued_recommendation(&queued).unwrap();
        drop(store);

        assert!(matches!(
            forge_now(&env).unwrap(),
            ForgeNowResult::DailyForgeCeilingReached {
                completed: 100,
                limit: 100
            }
        ));

        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.queued_recommendation_count().unwrap(), 1);
        assert!(store.latest_open_recommendation().unwrap().is_some());
    }

    fn codex_hook(event_name: &str, turn_id: Option<&str>) -> CodexHookEvent {
        CodexHookEvent {
            session_id: "session-1".into(),
            turn_id: turn_id.map(str::to_owned),
            cwd: "/work/svarog".into(),
            hook_event_name: event_name.into(),
            source: None,
            reason: None,
        }
    }

    #[test]
    fn process_event_promotes_queued_recommendation() {
        let env = test_env();
        let config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };
        crate::config::save(&env.paths, &config).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let rec = Recommendation {
            id: None,
            movement_id: "desk_posture_reset".into(),
            movement_name: "desk posture reset".into(),
            primary_muscle: "mobility".into(),
            muscles: vec!["neck".into()],
            reps: 4,
            weight_kg: None,
            estimated_seconds: 35,
            agent: Agent::Custom,
            project: None,
            side: None,
            created_at: chrono::Utc::now(),
        };
        store.insert_queued_recommendation(&rec).unwrap();

        assert!(!process_event(&env, event()).unwrap().recommended);
        let response = process_event(&env, event()).unwrap();

        assert!(response.recommended);
        assert_eq!(response.recommendation.unwrap().agent, Agent::Codex);
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
    }

    #[test]
    fn codex_prompts_repeat_notification_without_stacking_an_open_forge() {
        let env = test_env();
        let store = Store::open(&env.paths.database_file).unwrap();
        store.seed_movements().unwrap();
        let mut config = Config {
            recommender: Recommender {
                backend: RecommenderBackend::Local,
                ..Recommender::default()
            },
            ..Config::default()
        };
        config.preferences.desktop_notifications = true;
        crate::config::save(&env.paths, &config).unwrap();
        recommender::fill_recommendation_queue(&store, &config, &env.paths).unwrap();
        let delivered = RefCell::new(Vec::new());
        let notifier = |enabled: bool, title: &str, message: &str| {
            delivered
                .borrow_mut()
                .push((enabled, title.to_string(), message.to_string()));
            true
        };

        assert!(
            !process_event_with_notifier(&env, codex_prompt_event(), &notifier)
                .unwrap()
                .recommended
        );
        let first = process_event_with_notifier(&env, codex_prompt_event(), &notifier).unwrap();
        let offered_reminder =
            process_event_with_notifier(&env, codex_prompt_event(), &notifier).unwrap();

        assert!(first.recommended);
        assert!(!offered_reminder.recommended);
        let recommendation = first.recommendation.unwrap();
        assert_eq!(
            store.latest_open_recommendation().unwrap().unwrap().id,
            recommendation.id
        );

        let id = recommendation.id.unwrap();
        store.mark_recommendation(id, "active").unwrap();
        store
            .set_state(AppStateKind::Active, Some(id), None, None)
            .unwrap();
        let active_reminder =
            process_event_with_notifier(&env, codex_prompt_event(), &notifier).unwrap();

        assert!(!active_reminder.recommended);
        assert_eq!(
            store.latest_open_recommendation().unwrap().unwrap().id,
            Some(id)
        );
        let expected = (
            true,
            "Svarog".to_string(),
            format!("{} {}", recommendation.reps, recommendation.display_name()),
        );
        assert_eq!(
            delivered.into_inner(),
            vec![expected.clone(), expected.clone(), expected]
        );
    }

    #[test]
    fn duplicate_codex_prompt_is_one_opportunity() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();

        let prompt = codex_hook("UserPromptSubmit", Some("turn-1"));
        process_codex_hook(&env, prompt.clone()).unwrap();
        process_codex_hook(&env, prompt).unwrap();

        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        assert!(store.latest_open_recommendation().unwrap().is_none());
    }

    #[test]
    fn execution_turn_recommends_another_muscle_during_cooldown() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();

        process_codex_hook(&env, codex_hook("UserPromptSubmit", Some("warmup-turn"))).unwrap();
        process_codex_hook(&env, codex_hook("UserPromptSubmit", Some("plan-turn"))).unwrap();
        let store = Store::open(&env.paths.database_file).unwrap();
        let planning_forge = store.latest_open_recommendation().unwrap().unwrap();
        store
            .mark_recommendation(planning_forge.id.unwrap(), "done")
            .unwrap();
        store
            .record_set(&planning_forge, crate::models::SetStatus::Done)
            .unwrap();
        assert_eq!(store.state().unwrap().kind, AppStateKind::Cooldown);

        process_codex_hook(
            &env,
            codex_hook("UserPromptSubmit", Some("execution-warmup")),
        )
        .unwrap();
        process_codex_hook(&env, codex_hook("UserPromptSubmit", Some("execution-turn"))).unwrap();

        let execution_forge = store.latest_open_recommendation().unwrap().unwrap();
        assert_ne!(execution_forge.id, planning_forge.id);
        assert_ne!(
            execution_forge.primary_muscle,
            planning_forge.primary_muscle
        );
        assert_eq!(store.event_count().unwrap(), 4);
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
    }

    #[test]
    fn codex_stop_does_not_clear_open_forge() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();

        process_codex_hook(&env, codex_hook("UserPromptSubmit", Some("turn-0"))).unwrap();
        process_codex_hook(&env, codex_hook("UserPromptSubmit", Some("turn-1"))).unwrap();
        process_codex_hook(&env, codex_hook("Stop", Some("turn-1"))).unwrap();

        let store = Store::open(&env.paths.database_file).unwrap();
        assert!(store.latest_open_recommendation().unwrap().is_some());
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
    }

    #[tokio::test]
    async fn collector_serializes_concurrent_codex_prompts() {
        let env = test_env();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();
        let state = Arc::new(AppState {
            env: env.clone(),
            accept_codex: true,
            event_lock: Mutex::new(()),
        });
        let mut second = codex_hook("UserPromptSubmit", Some("turn-2"));
        second.session_id = "session-2".into();

        let (first_result, second_result) = tokio::join!(
            handle_codex_hook(
                State(state.clone()),
                Json(codex_hook("UserPromptSubmit", Some("turn-1")))
            ),
            handle_codex_hook(State(state), Json(second))
        );

        assert_eq!(first_result.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(second_result.unwrap(), StatusCode::NO_CONTENT);
        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 2);
        assert!(store.latest_open_recommendation().unwrap().is_some());
    }

    #[tokio::test]
    async fn standalone_daemon_ignores_codex_hooks() {
        let env = test_env();
        let state = Arc::new(AppState {
            env: env.clone(),
            accept_codex: false,
            event_lock: Mutex::new(()),
        });

        let result = handle_codex_hook(
            State(state),
            Json(codex_hook("UserPromptSubmit", Some("turn-1"))),
        )
        .await;

        assert_eq!(result.unwrap(), StatusCode::NO_CONTENT);
        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn collector_accepts_hooks_only_for_its_lifetime() {
        let mut env = test_env();
        env.daemon_addr = "127.0.0.1:0".parse().unwrap();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Local;
        crate::config::save(&env.paths, &config).unwrap();
        let collector = match Collector::start(&env).await {
            Ok(collector) => collector,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied) =>
            {
                return;
            }
            Err(error) => panic!("could not start test collector: {error:#}"),
        };
        let url = format!("http://{}/hooks/codex", collector.addr());
        let client = reqwest::Client::new();

        let response = client
            .post(&url)
            .json(&codex_hook("UserPromptSubmit", Some("turn-1")))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let store = Store::open(&env.paths.database_file).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        drop(store);
        collector.shutdown().await.unwrap();
        assert!(client
            .post(&url)
            .json(&codex_hook("UserPromptSubmit", Some("turn-2")))
            .send()
            .await
            .is_err());
    }

    #[test]
    fn tui_lock_allows_only_one_holder() {
        let env = test_env();
        env.paths.ensure().unwrap();
        let first = TuiLock::acquire(&env).unwrap();

        let error = TuiLock::acquire(&env).unwrap_err();
        assert!(error.to_string().contains("already running"));

        drop(first);
        TuiLock::acquire(&env).unwrap();
    }

    #[test]
    fn availability_check_rejects_running_tui() {
        let env = test_env();
        env.paths.ensure().unwrap();
        let _holder = TuiLock::acquire(&env).unwrap();

        let error = ensure_tui_available(&env).unwrap_err();

        assert!(error.to_string().contains("already running"));
    }
}
