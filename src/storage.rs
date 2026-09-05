use crate::config::{Config, UnitSystem};
#[cfg(test)]
use crate::models::FuelParseResult;
use crate::models::{
    Agent, AgentEvent, AppState, AppStateKind, CodexHookEvent, ForgeActivitySummary,
    ForgeActivityTotals, FuelEntry, Movement, MovementSidedness, MovementStatus, NutritionTotals,
    Recommendation, RecommendationSide, RecommenderTokenProvider, RecommenderTokenUsage,
    RecommenderTokenUsageSummary, SetStatus, TimedFuelEvent, TokenUsageTotals, WaterTotal,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const MUSCLE_COOLDOWN_MINUTES: i64 = 18;
pub const ML_PER_US_FL_OZ: f64 = 29.573_529_562_5;

#[derive(Debug, Clone, Serialize)]
pub struct SetSummary {
    pub movement_id: String,
    pub muscles: Vec<String>,
    pub status: String,
    pub reps: u32,
    pub prescribed_reps: u32,
    pub weight_kg: Option<f32>,
    pub agent: String,
    pub project: Option<String>,
    pub side: Option<RecommendationSide>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeSummary {
    pub movement_id: String,
    pub status: String,
    pub prescribed_reps: u32,
    pub actual_reps: u32,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForgeHistoryEntry {
    pub movement_name: String,
    pub status: String,
    pub reps: u32,
    pub weight_kg: Option<f32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightProgress {
    pub starting_kg: f32,
    pub current_kg: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoggedDayNutritionAverage {
    pub totals: NutritionTotals,
    pub logged_days: u32,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("configuring SQLite busy timeout")?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("securing {}", path.display()))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS movements (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                primary_muscle TEXT NOT NULL,
                muscles_json TEXT NOT NULL,
                equipment_json TEXT NOT NULL,
                base_reps INTEGER NOT NULL,
                estimated_seconds INTEGER NOT NULL,
                status TEXT NOT NULL,
                mobility INTEGER NOT NULL DEFAULT 0,
                sidedness TEXT NOT NULL DEFAULT 'bilateral'
            );

            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                profile_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                project TEXT,
                external_id TEXT,
                updated_at TEXT,
                ended_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                event TEXT NOT NULL,
                expected_duration_sec INTEGER NOT NULL,
                project TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS recommendations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                movement_id TEXT NOT NULL,
                movement_name TEXT NOT NULL,
                primary_muscle TEXT NOT NULL,
                muscles_json TEXT NOT NULL,
                reps INTEGER NOT NULL,
                weight_kg REAL,
                estimated_seconds INTEGER NOT NULL,
                side TEXT,
                agent TEXT NOT NULL,
                project TEXT,
                status TEXT NOT NULL DEFAULT 'recommended',
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recommendation_id INTEGER,
                movement_id TEXT NOT NULL,
                muscles_json TEXT NOT NULL DEFAULT '[]',
                status TEXT NOT NULL,
                reps INTEGER NOT NULL,
                weight_kg REAL,
                side TEXT,
                agent TEXT NOT NULL,
                project TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pain_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                movement_id TEXT NOT NULL,
                primary_muscle TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS exercise_catalog_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                catalog_revision TEXT NOT NULL,
                equipment_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS exercise_exclusions (
                exercise_id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS app_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                kind TEXT NOT NULL,
                current_recommendation_id INTEGER,
                cooldown_muscle TEXT,
                cooldown_until TEXT,
                suppress_until_event_count INTEGER,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS recommender_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL DEFAULT 'codex',
                input_tokens INTEGER NOT NULL,
                cached_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS recommender_token_usage_created_at
                ON recommender_token_usage(created_at);

            CREATE TABLE IF NOT EXISTS fuel_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                raw_text TEXT NOT NULL,
                items_json TEXT NOT NULL,
                calories REAL NOT NULL,
                protein_g REAL NOT NULL,
                carbohydrates_g REAL NOT NULL,
                fat_g REAL NOT NULL,
                fiber_g REAL NOT NULL,
                sugar_g REAL NOT NULL,
                sodium_mg REAL NOT NULL,
                potassium_mg REAL NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS fuel_entries_created_at
                ON fuel_entries(created_at);

            CREATE TABLE IF NOT EXISTS water_adjustments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                local_date TEXT NOT NULL,
                delta_ml REAL NOT NULL,
                delta_fl_oz REAL NOT NULL,
                total_ml REAL NOT NULL,
                total_fl_oz REAL NOT NULL,
                unit_system TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS water_adjustments_local_date
                ON water_adjustments(local_date, id);

            CREATE TABLE IF NOT EXISTS weight_checkins (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                weight_kg REAL NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        let _ = self.conn.execute(
            "ALTER TABLE users ADD COLUMN profile_json TEXT NOT NULL DEFAULT '{}'",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE sets ADD COLUMN muscles_json TEXT NOT NULL DEFAULT '[]'",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE movements ADD COLUMN sidedness TEXT NOT NULL DEFAULT 'bilateral'",
            [],
        );
        let _ = self
            .conn
            .execute("ALTER TABLE recommendations ADD COLUMN side TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE sets ADD COLUMN side TEXT", []);
        let _ = self.conn.execute(
            "ALTER TABLE app_state ADD COLUMN suppress_until_event_count INTEGER",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE recommender_token_usage ADD COLUMN provider TEXT NOT NULL DEFAULT 'codex'",
            [],
        );
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS recommender_token_usage_provider_created_at ON recommender_token_usage(provider, created_at)",
            [],
        )?;
        let _ = self
            .conn
            .execute("ALTER TABLE sessions ADD COLUMN external_id TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE sessions ADD COLUMN updated_at TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE sessions ADD COLUMN ended_at TEXT", []);
        self.conn.execute_batch(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS sessions_agent_external_id
                ON sessions(agent, external_id)
                WHERE external_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL REFERENCES sessions(id),
                external_id TEXT NOT NULL,
                project TEXT,
                started_at TEXT NOT NULL,
                stopped_at TEXT,
                UNIQUE(session_id, external_id)
            );
            "#,
        )?;
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO app_state
                (id, kind, current_recommendation_id, cooldown_muscle, cooldown_until, suppress_until_event_count, updated_at)
            VALUES (1, 'idle', NULL, NULL, NULL, NULL, ?1)
            "#,
            [Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn save_user_profile(&self, config: &Config) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO users (id, profile_json, created_at)
            VALUES (1, ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET profile_json = excluded.profile_json
            "#,
            params![
                serde_json::to_string(&config.profile)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn reset_all_data(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA secure_delete = ON;")
            .context("enabling secure SQLite deletion")?;
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            r#"
            DELETE FROM sets;
            DELETE FROM recommendations;
            DELETE FROM events;
            DELETE FROM turns;
            DELETE FROM sessions;
            DELETE FROM pain_events;
            DELETE FROM exercise_exclusions;
            DELETE FROM exercise_catalog_state;
            DELETE FROM recommender_token_usage;
            DELETE FROM fuel_entries;
            DELETE FROM water_adjustments;
            DELETE FROM users;
            DELETE FROM movements;
            DELETE FROM app_state;
            DELETE FROM sqlite_sequence
            WHERE name IN (
                'sets',
                'recommendations',
                'events',
                'turns',
                'sessions',
                'pain_events',
                'recommender_token_usage',
                'fuel_entries',
                'water_adjustments'
            );
            "#,
        )?;
        transaction.execute(
            r#"
            INSERT INTO app_state
                (id, kind, current_recommendation_id, cooldown_muscle, cooldown_until, suppress_until_event_count, updated_at)
            VALUES (1, 'idle', NULL, NULL, NULL, NULL, ?1)
            "#,
            [Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        self.conn
            .execute_batch(
                r#"
                PRAGMA wal_checkpoint(TRUNCATE);
                VACUUM;
                PRAGMA wal_checkpoint(TRUNCATE);
                "#,
            )
            .context("compacting reset SQLite data")?;
        Ok(())
    }

    #[cfg(test)]
    pub fn save_fuel_entry(
        &self,
        raw_text: &str,
        parsed: &FuelParseResult,
        provider: &str,
        model: &str,
        created_at: DateTime<Utc>,
    ) -> Result<i64> {
        let ids = self.save_fuel_batch(
            &[TimedFuelEvent {
                consumed_at: created_at,
                source_text: raw_text.to_string(),
                parsed: parsed.clone(),
            }],
            provider,
            model,
        )?;
        ids.into_iter().next().context("saving fuel entry")
    }

    pub fn save_fuel_batch(
        &self,
        events: &[TimedFuelEvent],
        provider: &str,
        model: &str,
    ) -> Result<Vec<i64>> {
        crate::fuel::validate_timed_events(events)
            .context("validating fuel batch before saving")?;
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let mut ids = Vec::with_capacity(events.len());
        for event in events {
            let totals = event.parsed.totals();
            transaction.execute(
                r#"
                INSERT INTO fuel_entries (
                    raw_text, items_json, calories, protein_g, carbohydrates_g, fat_g,
                    fiber_g, sugar_g, sodium_mg, potassium_mg, provider, model, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                "#,
                params![
                    event.source_text,
                    serde_json::to_string(&event.parsed)?,
                    totals.calories,
                    totals.protein_g,
                    totals.carbohydrates_g,
                    totals.fat_g,
                    totals.fiber_g,
                    totals.sugar_g,
                    totals.sodium_mg,
                    totals.potassium_mg,
                    provider,
                    model,
                    event.consumed_at.to_rfc3339(),
                ],
            )?;
            ids.push(transaction.last_insert_rowid());
        }
        transaction.commit()?;
        Ok(ids)
    }

    pub fn recent_fuel_entries(&self, limit: u32) -> Result<Vec<FuelEntry>> {
        self.fuel_entries_between(None, None, limit)
    }

    pub fn nutrition_totals_today(&self) -> Result<NutritionTotals> {
        self.nutrition_totals_today_at(Local::now())
    }

    pub fn nutrition_average_recent_logged_days(
        &self,
    ) -> Result<Option<LoggedDayNutritionAverage>> {
        self.nutrition_average_recent_logged_days_at(Local::now())
    }

    pub fn weight_progress(&self) -> Result<Option<WeightProgress>> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM weight_checkins", [], |row| row.get(0))?;
        if count < 2 {
            return Ok(None);
        }
        let starting_kg: f32 = self.conn.query_row(
            "SELECT weight_kg FROM weight_checkins ORDER BY id ASC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let current_kg: f32 = self.conn.query_row(
            "SELECT weight_kg FROM weight_checkins ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        if !starting_kg.is_finite()
            || !current_kg.is_finite()
            || starting_kg <= 0.0
            || current_kg <= 0.0
        {
            anyhow::bail!("stored weight check-ins are invalid");
        }
        Ok(Some(WeightProgress {
            starting_kg,
            current_kg,
        }))
    }

    pub fn nutrition_totals_today_at(&self, now: DateTime<Local>) -> Result<NutritionTotals> {
        let (start, end) = local_day_bounds_at(now)?;
        let totals = self.conn.query_row(
            r#"
            SELECT COALESCE(SUM(calories), 0.0),
                   COALESCE(SUM(protein_g), 0.0),
                   COALESCE(SUM(carbohydrates_g), 0.0),
                   COALESCE(SUM(fat_g), 0.0),
                   COALESCE(SUM(fiber_g), 0.0),
                   COALESCE(SUM(sugar_g), 0.0),
                   COALESCE(SUM(sodium_mg), 0.0),
                   COALESCE(SUM(potassium_mg), 0.0)
            FROM fuel_entries
            WHERE created_at >= ?1 AND created_at < ?2
            "#,
            params![start.to_rfc3339(), end.to_rfc3339()],
            |row| {
                Ok(NutritionTotals {
                    calories: row.get(0)?,
                    protein_g: row.get(1)?,
                    carbohydrates_g: row.get(2)?,
                    fat_g: row.get(3)?,
                    fiber_g: row.get(4)?,
                    sugar_g: row.get(5)?,
                    sodium_mg: row.get(6)?,
                    potassium_mg: row.get(7)?,
                })
            },
        )?;
        if totals
            .values()
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            anyhow::bail!("stored nutrition totals are invalid");
        }
        Ok(totals)
    }

    fn nutrition_average_recent_logged_days_at(
        &self,
        now: DateTime<Local>,
    ) -> Result<Option<LoggedDayNutritionAverage>> {
        const MAX_LOGGED_DAYS: usize = 7;

        let (_, end) = local_day_bounds_at(now)?;
        let mut statement = self.conn.prepare(
            r#"
            SELECT calories, protein_g, carbohydrates_g, fat_g, fiber_g,
                   sugar_g, sodium_mg, potassium_mg, created_at
            FROM fuel_entries
            WHERE created_at < ?1
            ORDER BY created_at DESC, id DESC
            "#,
        )?;
        let rows = statement.query_map([end.to_rfc3339()], |row| {
            Ok((
                NutritionTotals {
                    calories: row.get(0)?,
                    protein_g: row.get(1)?,
                    carbohydrates_g: row.get(2)?,
                    fat_g: row.get(3)?,
                    fiber_g: row.get(4)?,
                    sugar_g: row.get(5)?,
                    sodium_mg: row.get(6)?,
                    potassium_mg: row.get(7)?,
                },
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut dates = Vec::with_capacity(MAX_LOGGED_DAYS);
        let mut totals = NutritionTotals::default();
        for row in rows {
            let (entry, created_at) = row?;
            if entry
                .values()
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                anyhow::bail!("stored nutrition totals are invalid");
            }
            let date = DateTime::parse_from_rfc3339(&created_at)
                .context("parsing stored fuel timestamp")?
                .with_timezone(&Local)
                .date_naive();
            if !dates.contains(&date) {
                if dates.len() == MAX_LOGGED_DAYS {
                    break;
                }
                dates.push(date);
            }
            totals.add_assign(&entry);
        }

        if dates.is_empty() {
            return Ok(None);
        }
        totals.scale_assign(1.0 / dates.len() as f64);
        Ok(Some(LoggedDayNutritionAverage {
            totals,
            logged_days: dates.len() as u32,
        }))
    }

    fn fuel_entries_between(
        &self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<FuelEntry>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, raw_text, items_json, calories, protein_g, carbohydrates_g,
                   fat_g, fiber_g, sugar_g, sodium_mg, potassium_mg,
                   provider, model, created_at
            FROM fuel_entries
            WHERE (?1 IS NULL OR created_at >= ?1)
              AND (?2 IS NULL OR created_at < ?2)
            ORDER BY created_at DESC, id DESC
            LIMIT ?3
            "#,
        )?;
        let start = start.map(|value| value.to_rfc3339());
        let end = end.map(|value| value.to_rfc3339());
        let rows = stmt.query_map(params![start, end, limit], fuel_entry_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading fuel entries")
    }

    pub fn delete_fuel_entry(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM fuel_entries WHERE id = ?1", [id])?
            > 0)
    }

    pub fn water_total_today(&self) -> Result<WaterTotal> {
        self.water_total_at(Local::now())
    }

    pub fn water_total_at(&self, now: DateTime<Local>) -> Result<WaterTotal> {
        let date = now.date_naive().to_string();
        let milliliters = self.conn.query_row(
            "SELECT COALESCE(SUM(delta_ml), 0.0) FROM water_adjustments WHERE local_date = ?1",
            [date],
            |row| row.get::<_, f64>(0),
        )?;
        if !milliliters.is_finite() || milliliters < 0.0 {
            anyhow::bail!("stored water total is invalid");
        }
        Ok(WaterTotal {
            milliliters,
            fluid_ounces: milliliters / ML_PER_US_FL_OZ,
        })
    }

    pub fn adjust_water_today(
        &self,
        requested_delta_ml: f64,
        unit_system: UnitSystem,
    ) -> Result<WaterTotal> {
        self.adjust_water_at(requested_delta_ml, unit_system, Local::now())
    }

    pub fn adjust_water_at(
        &self,
        requested_delta_ml: f64,
        unit_system: UnitSystem,
        now: DateTime<Local>,
    ) -> Result<WaterTotal> {
        if !requested_delta_ml.is_finite() || requested_delta_ml == 0.0 {
            anyhow::bail!("water adjustment must be a finite non-zero value");
        }
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let local_date = now.date_naive().to_string();
        let current: f64 = transaction.query_row(
            "SELECT COALESCE(SUM(delta_ml), 0.0) FROM water_adjustments WHERE local_date = ?1",
            [&local_date],
            |row| row.get(0),
        )?;
        if !current.is_finite() || current < 0.0 {
            anyhow::bail!("stored water total is invalid");
        }
        let total_ml = (current + requested_delta_ml).max(0.0);
        if !total_ml.is_finite() {
            anyhow::bail!("water total is too large");
        }
        let actual_delta_ml = total_ml - current;
        if actual_delta_ml == 0.0 {
            return Ok(WaterTotal {
                milliliters: current,
                fluid_ounces: current / ML_PER_US_FL_OZ,
            });
        }
        transaction.execute(
            r#"
            INSERT INTO water_adjustments (
                local_date, delta_ml, delta_fl_oz, total_ml, total_fl_oz,
                unit_system, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                local_date,
                actual_delta_ml,
                actual_delta_ml / ML_PER_US_FL_OZ,
                total_ml,
                total_ml / ML_PER_US_FL_OZ,
                unit_system.to_string(),
                now.with_timezone(&Utc).to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(WaterTotal {
            milliliters: total_ml,
            fluid_ounces: total_ml / ML_PER_US_FL_OZ,
        })
    }

    pub fn record_recommender_token_usage(
        &self,
        provider: RecommenderTokenProvider,
        usage: &RecommenderTokenUsage,
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO recommender_token_usage
                (provider, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                provider.as_str(),
                i64::try_from(usage.input_tokens).context("input token count is too large")?,
                i64::try_from(usage.cached_input_tokens)
                    .context("cached input token count is too large")?,
                i64::try_from(usage.output_tokens).context("output token count is too large")?,
                i64::try_from(usage.reasoning_output_tokens)
                    .context("reasoning output token count is too large")?,
                created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn recommender_token_usage_summary_for(
        &self,
        provider: RecommenderTokenProvider,
    ) -> Result<RecommenderTokenUsageSummary> {
        self.recommender_token_usage_summary_for_at(provider, Local::now())
    }

    fn recommender_token_usage_summary_for_at(
        &self,
        provider: RecommenderTokenProvider,
        now: DateTime<Local>,
    ) -> Result<RecommenderTokenUsageSummary> {
        let (today_start, week_start) = local_period_starts_at(now)?;
        Ok(RecommenderTokenUsageSummary {
            today: self.recommender_token_usage_since(provider, today_start)?,
            week: self.recommender_token_usage_since(provider, week_start)?,
        })
    }

    pub fn completed_forge_summary(&self) -> Result<ForgeActivitySummary> {
        self.completed_forge_summary_at(Local::now())
    }

    fn completed_forge_summary_at(&self, now: DateTime<Local>) -> Result<ForgeActivitySummary> {
        let (today_start, week_start) = local_period_starts_at(now)?;
        Ok(ForgeActivitySummary {
            today: self.completed_forges_since(today_start)?,
            week: self.completed_forges_since(week_start)?,
        })
    }

    pub fn recent_forge_history(&self, limit: u32) -> Result<Vec<ForgeHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                COALESCE(r.movement_name, m.name, s.movement_id),
                s.status,
                s.reps,
                s.weight_kg,
                s.created_at
            FROM sets s
            LEFT JOIN recommendations r ON r.id = s.recommendation_id
            LEFT JOIN movements m ON m.id = s.movement_id
            WHERE s.status IN ('done', 'skipped', 'pain')
            ORDER BY s.created_at DESC, s.id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map([i64::from(limit)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut history = Vec::new();
        for row in rows {
            let (movement_name, status, reps, weight_kg, created_at) = row?;
            history.push(ForgeHistoryEntry {
                movement_name,
                status,
                reps: reps as u32,
                weight_kg: weight_kg.map(|value| value as f32),
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .with_context(|| format!("parsing forge timestamp {created_at}"))?
                    .with_timezone(&Utc),
            });
        }
        Ok(history)
    }

    fn recommender_token_usage_since(
        &self,
        provider: RecommenderTokenProvider,
        start: DateTime<Utc>,
    ) -> Result<TokenUsageTotals> {
        self.conn
            .query_row(
                r#"
                SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
                FROM recommender_token_usage
                WHERE provider = ?1 AND created_at >= ?2
                "#,
                params![provider.as_str(), start.to_rfc3339()],
                |row| {
                    Ok(TokenUsageTotals {
                        input_tokens: row.get::<_, i64>(0)? as u64,
                        output_tokens: row.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .context("loading recommender token usage")
    }

    fn completed_forges_since(&self, start: DateTime<Utc>) -> Result<ForgeActivityTotals> {
        self.conn
            .query_row(
                r#"
                SELECT COUNT(*), COALESCE(SUM(reps), 0)
                FROM sets
                WHERE status = 'done' AND created_at >= ?1
                "#,
                [start.to_rfc3339()],
                |row| {
                    Ok(ForgeActivityTotals {
                        forges: row.get::<_, i64>(0)? as u64,
                        reps: row.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .context("loading completed forge activity")
    }

    pub fn insert_session(&self, agent: Agent, project: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (agent, project, created_at) VALUES (?1, ?2, ?3)",
            params![agent.as_str(), project, Utc::now().to_rfc3339()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn record_codex_session(&self, event: &CodexHookEvent) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let project = event.project();
        self.conn.execute(
            r#"
            INSERT INTO sessions (agent, project, external_id, updated_at, ended_at, created_at)
            VALUES ('codex', ?1, ?2, ?3, NULL, ?3)
            ON CONFLICT(agent, external_id) WHERE external_id IS NOT NULL DO UPDATE SET
                project = excluded.project,
                updated_at = excluded.updated_at,
                ended_at = NULL
            "#,
            params![project.as_deref(), &event.session_id, &now],
        )?;
        self.conn
            .query_row(
                "SELECT id FROM sessions WHERE agent = 'codex' AND external_id = ?1",
                [&event.session_id],
                |row| row.get(0),
            )
            .context("loading Codex session")
    }

    pub fn record_codex_prompt(&self, event: &CodexHookEvent) -> Result<bool> {
        let Some(turn_id) = event.turn_id.as_deref() else {
            return Ok(false);
        };
        let session_id = self.record_codex_session(event)?;
        let changed = self.conn.execute(
            r#"
            INSERT OR IGNORE INTO turns
                (session_id, external_id, project, started_at, stopped_at)
            VALUES (?1, ?2, ?3, ?4, NULL)
            "#,
            params![
                session_id,
                turn_id,
                event.project().as_deref(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn record_codex_stop(&self, event: &CodexHookEvent) -> Result<()> {
        let Some(turn_id) = event.turn_id.as_deref() else {
            return Ok(());
        };
        let session_id = self.record_codex_session(event)?;
        self.conn.execute(
            "UPDATE turns SET stopped_at = ?1 WHERE session_id = ?2 AND external_id = ?3",
            params![Utc::now().to_rfc3339(), session_id, turn_id],
        )?;
        Ok(())
    }

    pub fn record_codex_session_end(&self, event: &CodexHookEvent) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE sessions
            SET updated_at = ?1, ended_at = ?1
            WHERE agent = 'codex' AND external_id = ?2
            "#,
            params![now, &event.session_id],
        )?;
        Ok(())
    }

    pub fn seed_movements(&self) -> Result<()> {
        let existing: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM movements", [], |row| row.get(0))?;
        if existing > 0 {
            return Ok(());
        }
        let equipment = vec!["bodyweight".to_string()];
        self.replace_movement_pool(&crate::exercise_catalog::movements_for_equipment(
            &equipment,
        ))
    }

    pub fn replace_movement_pool(&self, movements: &[Movement]) -> Result<()> {
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("starting exercise-pool replacement")?;
        Self::replace_movement_pool_in_transaction(&transaction, movements)?;
        transaction
            .commit()
            .context("committing exercise-pool replacement")
    }

    pub fn apply_user_profile_and_movement_pool(
        &self,
        config: &Config,
        previous_weight_kg: Option<f32>,
        movements: &[Movement],
        equipment_filter_json: &str,
    ) -> Result<()> {
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("starting settings update")?;
        transaction.execute(
            r#"
            INSERT INTO users (id, profile_json, created_at)
            VALUES (1, ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET profile_json = excluded.profile_json
            "#,
            params![
                serde_json::to_string(&config.profile)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        Self::record_weight_checkin_in_transaction(
            &transaction,
            previous_weight_kg,
            config.profile.weight_kg,
        )?;
        Self::replace_movement_pool_in_transaction(&transaction, movements)?;
        transaction.execute(
            "UPDATE exercise_catalog_state SET equipment_json = ?1 WHERE id = 1",
            [equipment_filter_json],
        )?;
        transaction.commit().context("committing settings update")
    }

    fn record_weight_checkin_in_transaction(
        transaction: &Transaction<'_>,
        previous_weight_kg: Option<f32>,
        current_weight_kg: Option<f32>,
    ) -> Result<()> {
        let valid = |weight: Option<f32>| weight.filter(|value| value.is_finite() && *value > 0.0);
        let previous_weight_kg = valid(previous_weight_kg);
        let current_weight_kg = valid(current_weight_kg);
        let count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM weight_checkins", [], |row| row.get(0))?;
        if count == 0 {
            if let Some(weight) = previous_weight_kg {
                transaction.execute(
                    "INSERT INTO weight_checkins (weight_kg, created_at) VALUES (?1, ?2)",
                    params![weight, Utc::now().to_rfc3339()],
                )?;
            }
        }
        let latest_weight_kg = transaction
            .query_row(
                "SELECT weight_kg FROM weight_checkins ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, f32>(0),
            )
            .optional()?;
        if let Some(weight) = current_weight_kg.filter(|weight| {
            latest_weight_kg.is_none_or(|latest| (latest - *weight).abs() > 0.000_1)
        }) {
            transaction.execute(
                "INSERT INTO weight_checkins (weight_kg, created_at) VALUES (?1, ?2)",
                params![weight, Utc::now().to_rfc3339()],
            )?;
        }
        Ok(())
    }

    fn replace_movement_pool_in_transaction(
        transaction: &Transaction<'_>,
        movements: &[Movement],
    ) -> Result<()> {
        transaction.execute("DELETE FROM movements", [])?;
        {
            let mut statement = transaction.prepare(
                r#"
                INSERT INTO movements
                    (id, name, primary_muscle, muscles_json, equipment_json, base_reps, estimated_seconds, status, mobility, sidedness)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    CASE WHEN EXISTS (SELECT 1 FROM pain_events WHERE movement_id = ?1)
                         THEN 'blocked' ELSE ?8 END,
                    ?9, ?10)
                "#,
            )?;
            for movement in movements {
                statement.execute(params![
                    &movement.id,
                    &movement.name,
                    &movement.primary_muscle,
                    serde_json::to_string(&movement.muscles)?,
                    serde_json::to_string(&movement.equipment)?,
                    i64::from(movement.base_reps),
                    i64::from(movement.estimated_seconds),
                    status_to_str(movement.status),
                    movement.mobility as i32,
                    sidedness_to_str(movement.sidedness),
                ])?;
            }
        }
        let equipment = movements
            .iter()
            .flat_map(|movement| movement.equipment.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        let equipment = equipment
            .into_iter()
            .map(|kind| serde_json::json!({ "kind": kind, "weights_kg": [] }))
            .collect::<Vec<_>>();
        transaction.execute(
            r#"
            INSERT INTO exercise_catalog_state (id, catalog_revision, equipment_json, updated_at)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                catalog_revision = excluded.catalog_revision,
                updated_at = excluded.updated_at
            "#,
            params![
                crate::exercise_catalog::movement_pool_revision(),
                serde_json::to_string(&equipment)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        transaction.execute(
            r#"
            UPDATE recommendations
            SET status = 'retired'
            WHERE status IN ('queued', 'recommended', 'active')
              AND movement_id NOT IN (SELECT id FROM movements)
            "#,
            [],
        )?;
        transaction.execute(
            r#"
            UPDATE app_state
            SET kind = 'idle', current_recommendation_id = NULL,
                cooldown_muscle = NULL, cooldown_until = NULL, updated_at = ?1
            WHERE current_recommendation_id IN (
                SELECT id FROM recommendations WHERE status = 'retired'
            )
            "#,
            [Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn clear_exercise_exclusions(&self) -> Result<()> {
        self.conn.execute("DELETE FROM exercise_exclusions", [])?;
        Ok(())
    }

    pub fn exercise_catalog_is_current(&self) -> Result<bool> {
        let revision = self
            .conn
            .query_row(
                "SELECT catalog_revision FROM exercise_catalog_state WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(revision.as_deref() == Some(crate::exercise_catalog::movement_pool_revision()))
    }

    pub fn save_exercise_filter<T: serde::Serialize>(&self, filter: &T) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO exercise_catalog_state (id, catalog_revision, equipment_json, updated_at)
            VALUES (1, 'pending', ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET
                catalog_revision = 'pending',
                equipment_json = excluded.equipment_json,
                updated_at = excluded.updated_at
            "#,
            params![serde_json::to_string(filter)?, Utc::now().to_rfc3339(),],
        )?;
        Ok(())
    }

    pub fn exercise_filter<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>> {
        let json = self
            .conn
            .query_row(
                "SELECT equipment_json FROM exercise_catalog_state WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).context("parsing stored exercise filter"))
            .transpose()
    }

    pub fn exclude_exercise(&self, exercise_id: &str) -> Result<()> {
        if crate::exercise_catalog::find(exercise_id).is_none() {
            anyhow::bail!("unknown canonical exercise id: {exercise_id}");
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO exercise_exclusions (exercise_id, created_at) VALUES (?1, ?2)",
            params![exercise_id, Utc::now().to_rfc3339()],
        )?;
        self.conn.execute(
            "UPDATE recommendations SET status = 'retired' WHERE movement_id = ?1 AND status = 'queued'",
            [exercise_id],
        )?;
        Ok(())
    }

    pub fn removed_exercise_ids(&self) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT exercise_id FROM exercise_exclusions ORDER BY created_at, exercise_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading removed exercises")
    }

    pub fn restore_exercise(&self, exercise_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM exercise_exclusions WHERE exercise_id = ?1",
            [exercise_id],
        )? > 0)
    }

    pub fn upsert_movement(&self, movement: &Movement) -> Result<()> {
        let status = status_to_str(movement.status);
        self.conn.execute(
            r#"
            INSERT INTO movements
                (id, name, primary_muscle, muscles_json, equipment_json, base_reps, estimated_seconds, status, mobility, sidedness)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                primary_muscle = excluded.primary_muscle,
                muscles_json = excluded.muscles_json,
                equipment_json = excluded.equipment_json,
                base_reps = excluded.base_reps,
                estimated_seconds = excluded.estimated_seconds,
                status = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM pain_events
                        WHERE movement_id = excluded.id
                    ) THEN 'blocked'
                    ELSE excluded.status
                END,
                mobility = excluded.mobility,
                sidedness = excluded.sidedness
            "#,
            params![
                &movement.id,
                &movement.name,
                &movement.primary_muscle,
                serde_json::to_string(&movement.muscles)?,
                serde_json::to_string(&movement.equipment)?,
                i64::from(movement.base_reps),
                i64::from(movement.estimated_seconds),
                status,
                movement.mobility as i32,
                sidedness_to_str(movement.sidedness),
            ],
        )?;
        Ok(())
    }

    pub fn movements(&self) -> Result<Vec<Movement>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, name, primary_muscle, muscles_json, equipment_json,
                   base_reps, estimated_seconds, status, mobility, sidedness
            FROM movements
            WHERE id NOT IN (SELECT exercise_id FROM exercise_exclusions)
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(7)?;
            Ok(Movement {
                id: row.get(0)?,
                name: row.get(1)?,
                primary_muscle: row.get(2)?,
                muscles: serde_json::from_str::<Vec<String>>(&row.get::<_, String>(3)?)
                    .unwrap_or_default(),
                equipment: serde_json::from_str::<Vec<String>>(&row.get::<_, String>(4)?)
                    .unwrap_or_default(),
                base_reps: row.get::<_, i64>(5)? as u32,
                estimated_seconds: row.get::<_, i64>(6)? as u32,
                status: str_to_status(&status),
                mobility: row.get::<_, i32>(8)? == 1,
                sidedness: sidedness_from_str(&row.get::<_, String>(9)?),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading movements")
    }

    pub fn insert_event(&self, event: &AgentEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO events (agent, event, expected_duration_sec, project, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.agent.as_str(),
                &event.event,
                i64::from(event.expected_duration_sec),
                event.project.as_deref(),
                event.created_at.to_rfc3339(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_recommendation(&self, rec: &Recommendation) -> Result<i64> {
        self.insert_recommendation_with_status(rec, "recommended")
    }

    pub fn insert_queued_recommendation(&self, rec: &Recommendation) -> Result<i64> {
        self.insert_recommendation_with_status(rec, "queued")
    }

    fn insert_recommendation_with_status(&self, rec: &Recommendation, status: &str) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO recommendations
                (movement_id, movement_name, primary_muscle, muscles_json, reps, weight_kg, estimated_seconds, side, agent, project, status, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                &rec.movement_id,
                &rec.movement_name,
                &rec.primary_muscle,
                serde_json::to_string(&rec.muscles)?,
                i64::from(rec.reps),
                rec.weight_kg.map(f64::from),
                i64::from(rec.estimated_seconds),
                rec.side.map(side_to_str),
                rec.agent.as_str(),
                rec.project.as_deref(),
                status,
                rec.created_at.to_rfc3339(),
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if status == "recommended" {
            self.set_state(AppStateKind::Recommendation, Some(id), None, None)?;
        }
        Ok(id)
    }

    pub fn queued_recommendation_count(&self) -> Result<u32> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM recommendations WHERE status = 'queued'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count as u32),
            )
            .context("counting queued recommendations")
    }

    pub fn queued_recommendations(&self) -> Result<Vec<Recommendation>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT id, movement_id, movement_name, primary_muscle, muscles_json, reps,
                   weight_kg, estimated_seconds, side, agent, project, created_at
            FROM recommendations
            WHERE status = 'queued'
            ORDER BY id ASC
            "#,
        )?;
        let rows = statement.query_map([], recommendation_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading queued recommendations")
    }

    pub fn clear_queued_recommendations(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM recommendations WHERE status = 'queued'", [])
            .context("clearing queued recommendations")?;
        Ok(())
    }

    pub fn replace_queued_recommendations(
        &mut self,
        recommendations: &[Recommendation],
    ) -> Result<()> {
        let transaction = self
            .conn
            .transaction()
            .context("starting queued recommendation replacement")?;
        transaction
            .execute("DELETE FROM recommendations WHERE status = 'queued'", [])
            .context("clearing queued recommendations")?;
        {
            let mut statement = transaction.prepare(
                r#"
                INSERT INTO recommendations
                    (movement_id, movement_name, primary_muscle, muscles_json, reps, weight_kg, estimated_seconds, side, agent, project, status, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued', ?11)
                "#,
            )?;
            for rec in recommendations {
                statement.execute(params![
                    &rec.movement_id,
                    &rec.movement_name,
                    &rec.primary_muscle,
                    serde_json::to_string(&rec.muscles)?,
                    i64::from(rec.reps),
                    rec.weight_kg.map(f64::from),
                    i64::from(rec.estimated_seconds),
                    rec.side.map(side_to_str),
                    rec.agent.as_str(),
                    rec.project.as_deref(),
                    rec.created_at.to_rfc3339(),
                ])?;
            }
        }
        transaction
            .commit()
            .context("committing queued recommendation replacement")
    }

    pub fn promote_next_queued_recommendation(
        &self,
        agent: Agent,
        project: Option<&str>,
    ) -> Result<Option<Recommendation>> {
        self.promote_next_queued_recommendation_with_metadata(Some(agent), project)
    }

    pub fn promote_next_queued_recommendation_preserving_metadata(
        &self,
    ) -> Result<Option<Recommendation>> {
        self.promote_next_queued_recommendation_with_metadata(None, None)
    }

    fn promote_next_queued_recommendation_with_metadata(
        &self,
        agent: Option<Agent>,
        project: Option<&str>,
    ) -> Result<Option<Recommendation>> {
        let mut statement = self.conn.prepare(
            "SELECT id, primary_muscle, side FROM recommendations WHERE status = 'queued' ORDER BY id ASC",
        )?;
        let candidates = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    side_from_str(row.get(2)?),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let mut recovered_id = None;
        for (id, primary_muscle, side) in candidates {
            if self.muscle_side_recovered(&primary_muscle, side, MUSCLE_COOLDOWN_MINUTES)? {
                recovered_id = Some(id);
                break;
            }
        }
        let Some(id) = recovered_id else {
            return Ok(None);
        };

        let now = Utc::now().to_rfc3339();
        if let Some(agent) = agent {
            self.conn.execute(
                "UPDATE recommendations SET status = 'recommended', agent = ?1, project = ?2, created_at = ?3 WHERE id = ?4",
                params![agent.as_str(), project, now, id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE recommendations SET status = 'recommended', created_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
        }
        self.set_state(AppStateKind::Recommendation, Some(id), None, None)?;
        self.recommendation_by_id(id)
    }

    fn recommendation_by_id(&self, id: i64) -> Result<Option<Recommendation>> {
        self.conn
            .query_row(
                r#"
                SELECT id, movement_id, movement_name, primary_muscle, muscles_json, reps, weight_kg, estimated_seconds, side, agent, project, created_at
                FROM recommendations
                WHERE id = ?1
                "#,
                [id],
                recommendation_from_row,
            )
            .optional()
            .context("loading recommendation")
    }

    pub fn latest_open_recommendation(&self) -> Result<Option<Recommendation>> {
        self.conn
            .query_row(
                r#"
                SELECT id, movement_id, movement_name, primary_muscle, muscles_json, reps, weight_kg, estimated_seconds, side, agent, project, created_at
                FROM recommendations
                WHERE status IN ('recommended', 'active')
                ORDER BY id DESC
                LIMIT 1
                "#,
                [],
                recommendation_from_row,
            )
            .optional()
            .context("loading latest recommendation")
    }

    pub fn mark_recommendation(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE recommendations SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    pub fn record_set(&self, rec: &Recommendation, status: SetStatus) -> Result<()> {
        self.record_set_with_reps(rec, status, rec.reps)
    }

    pub fn record_set_with_reps(
        &self,
        rec: &Recommendation,
        status: SetStatus,
        reps: u32,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO sets (recommendation_id, movement_id, muscles_json, status, reps, weight_kg, side, agent, project, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                rec.id,
                &rec.movement_id,
                serde_json::to_string(&rec.muscles)?,
                status.as_str(),
                i64::from(reps),
                rec.weight_kg.map(f64::from),
                rec.side.map(side_to_str),
                rec.agent.as_str(),
                rec.project.as_deref(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        if status == SetStatus::Pain {
            self.conn.execute(
                "INSERT INTO pain_events (movement_id, primary_muscle, created_at) VALUES (?1, ?2, ?3)",
                params![&rec.movement_id, &rec.primary_muscle, Utc::now().to_rfc3339()],
            )?;
            self.conn.execute(
                "UPDATE movements SET status = 'blocked' WHERE id = ?1",
                params![&rec.movement_id],
            )?;
        }
        match status {
            SetStatus::Done => {
                let cooldown_until =
                    Utc::now() + chrono::Duration::minutes(MUSCLE_COOLDOWN_MINUTES);
                self.set_state(
                    AppStateKind::Cooldown,
                    None,
                    Some(&rec.primary_muscle),
                    Some(cooldown_until),
                )?;
            }
            SetStatus::Skipped | SetStatus::Pain => {
                self.set_state(AppStateKind::Idle, None, None, None)?;
            }
            SetStatus::Started => {}
        }
        Ok(())
    }

    pub fn suppress_next_opportunities(&self, opportunities: u32) -> Result<()> {
        let until = self.event_count()?.saturating_add(opportunities);
        self.conn.execute(
            "UPDATE app_state SET suppress_until_event_count = ?1, updated_at = ?2 WHERE id = 1",
            params![i64::from(until), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn fatigue_suppression_active(&self) -> Result<bool> {
        let Some(until) = self.current_suppression()? else {
            return Ok(false);
        };
        if self.event_count()? <= until {
            return Ok(true);
        }
        self.clear_fatigue_suppression()?;
        Ok(false)
    }

    fn current_suppression(&self) -> Result<Option<u32>> {
        self.conn
            .query_row(
                "SELECT suppress_until_event_count FROM app_state WHERE id = 1",
                [],
                |row| {
                    row.get::<_, Option<i64>>(0)
                        .map(|value| value.map(|value| value as u32))
                },
            )
            .context("loading fatigue suppression")
    }

    fn clear_fatigue_suppression(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE app_state SET suppress_until_event_count = NULL, updated_at = ?1 WHERE id = 1",
            [Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_state(
        &self,
        kind: AppStateKind,
        recommendation_id: Option<i64>,
        cooldown_muscle: Option<&str>,
        cooldown_until: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let suppress_until_event_count = self.current_suppression()?;
        self.conn.execute(
            r#"
            INSERT INTO app_state
                (id, kind, current_recommendation_id, cooldown_muscle, cooldown_until, suppress_until_event_count, updated_at)
            VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                current_recommendation_id = excluded.current_recommendation_id,
                cooldown_muscle = excluded.cooldown_muscle,
                cooldown_until = excluded.cooldown_until,
                suppress_until_event_count = excluded.suppress_until_event_count,
                updated_at = excluded.updated_at
            "#,
            params![
                kind.as_str(),
                recommendation_id,
                cooldown_muscle,
                cooldown_until.map(|dt| dt.to_rfc3339()),
                suppress_until_event_count.map(i64::from),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn state(&self) -> Result<AppState> {
        let mut state = self
            .conn
            .query_row(
                "SELECT kind, current_recommendation_id, cooldown_muscle, cooldown_until, suppress_until_event_count, updated_at FROM app_state WHERE id = 1",
                [],
                |row| {
                    let kind: String = row.get(0)?;
                    let cooldown_until: Option<String> = row.get(3)?;
                    let updated_at: String = row.get(5)?;
                    Ok(AppState {
                        kind: kind.parse().unwrap_or(AppStateKind::Idle),
                        current_recommendation_id: row.get(1)?,
                        cooldown_muscle: row.get(2)?,
                        cooldown_until: cooldown_until
                            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
                            .map(|dt| dt.with_timezone(&Utc)),
                        suppress_until_event_count: row
                            .get::<_, Option<i64>>(4)?
                            .map(|value| value as u32),
                        updated_at: DateTime::parse_from_rfc3339(&updated_at)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    })
                },
            )
            .context("loading app state")?;

        if state.kind == AppStateKind::Cooldown
            && state
                .cooldown_until
                .as_ref()
                .is_some_and(|until| *until <= Utc::now())
        {
            self.set_state(AppStateKind::Idle, None, None, None)?;
            state.kind = AppStateKind::Idle;
            state.cooldown_muscle = None;
            state.cooldown_until = None;
        }

        if state
            .suppress_until_event_count
            .is_some_and(|until| self.event_count().is_ok_and(|count| count > until))
        {
            self.clear_fatigue_suppression()?;
            state.suppress_until_event_count = None;
        }

        Ok(state)
    }

    pub fn muscle_recovered(&self, muscle: &str, recovery_minutes: i64) -> Result<bool> {
        self.muscle_side_recovered(muscle, None, recovery_minutes)
    }

    fn muscle_side_recovered(
        &self,
        muscle: &str,
        side: Option<RecommendationSide>,
        recovery_minutes: i64,
    ) -> Result<bool> {
        let side = match side {
            Some(RecommendationSide::Left) => Some("left"),
            Some(RecommendationSide::Right) => Some("right"),
            Some(RecommendationSide::Bilateral) | None => None,
        };
        let last_done = self
            .conn
            .query_row(
                r#"
                SELECT s.created_at
                FROM sets s
                JOIN recommendations r ON r.id = s.recommendation_id
                WHERE s.status = 'done' AND r.primary_muscle = ?1
                  AND (
                    ?2 IS NULL
                    OR s.side IS NULL
                    OR s.side = 'bilateral'
                    OR s.side = ?2
                  )
                ORDER BY s.id DESC
                LIMIT 1
                "#,
                params![muscle, side],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("loading muscle recovery")?;

        let Some(last_done) = last_done else {
            return Ok(true);
        };
        let last_done = DateTime::parse_from_rfc3339(&last_done)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Ok(Utc::now() - last_done >= chrono::Duration::minutes(recovery_minutes))
    }

    pub fn last_done_muscle(&self) -> Result<Option<String>> {
        self.conn
            .query_row(
                r#"
                SELECT r.primary_muscle
                FROM sets s
                JOIN recommendations r ON r.id = s.recommendation_id
                WHERE s.status = 'done'
                ORDER BY s.id DESC
                LIMIT 1
                "#,
                [],
                |row| row.get(0),
            )
            .optional()
            .context("loading last done muscle")
    }

    pub fn today_set_count(&self) -> Result<u32> {
        self.today_set_count_at(Local::now())
    }

    fn today_set_count_at(&self, now: DateTime<Local>) -> Result<u32> {
        let (start, end) = local_day_bounds_at(now)?;
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM sets WHERE created_at >= ?1 AND created_at < ?2 AND status = 'done'",
                params![start.to_rfc3339(), end.to_rfc3339()],
                |row| row.get::<_, i64>(0).map(|count| count as u32),
            )
            .context("counting today's sets")
    }

    pub fn intervention_count(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM sets", [], |row| {
                row.get::<_, i64>(0).map(|count| count as u32)
            })
            .context("counting interventions")
    }

    pub fn event_count(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                row.get::<_, i64>(0).map(|count| count as u32)
            })
            .context("counting events")
    }

    pub fn stats_today(&self) -> Result<(u32, u32, u32)> {
        self.stats_today_at(Local::now())
    }

    fn stats_today_at(&self, now: DateTime<Local>) -> Result<(u32, u32, u32)> {
        let (start, end) = local_day_bounds_at(now)?;
        self.conn
            .query_row(
                r#"
                SELECT
                    SUM(CASE WHEN status = 'done' THEN 1 ELSE 0 END),
                    COALESCE(SUM(CASE WHEN status = 'done' THEN reps ELSE 0 END), 0),
                    SUM(CASE WHEN status IN ('skipped', 'pain') THEN 1 ELSE 0 END)
                FROM sets
                WHERE created_at >= ?1 AND created_at < ?2
                "#,
                params![start.to_rfc3339(), end.to_rfc3339()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.unwrap_or(0) as u32,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u32,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                    ))
                },
            )
            .context("loading today's stats")
    }

    pub fn recent_movement_outcomes(
        &self,
        movement_id: &str,
        limit: u32,
    ) -> Result<Vec<OutcomeSummary>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT s.movement_id, s.status, COALESCE(r.reps, s.reps), s.reps, s.created_at
            FROM sets s
            LEFT JOIN recommendations r ON r.id = s.recommendation_id
            WHERE s.movement_id = ?1 AND s.status IN ('done', 'skipped', 'pain')
            ORDER BY s.created_at DESC, s.id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = statement.query_map(params![movement_id, i64::from(limit)], |row| {
            Ok(OutcomeSummary {
                movement_id: row.get(0)?,
                status: row.get(1)?,
                prescribed_reps: row.get::<_, i64>(2)? as u32,
                actual_reps: row.get::<_, i64>(3)? as u32,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading movement outcomes")
    }

    pub fn events_since_last_outcome(&self) -> Result<usize> {
        let last: Option<String> = self
            .conn
            .query_row(
                "SELECT created_at FROM sets WHERE status IN ('done', 'skipped', 'pain') ORDER BY created_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match last {
            Some(created_at) => self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE created_at > ?1",
                    [created_at],
                    |row| row.get::<_, i64>(0).map(|count| count as usize),
                )
                .context("counting movement opportunities"),
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM events", [], |row| {
                    row.get::<_, i64>(0).map(|count| count as usize)
                })
                .context("counting initial movement opportunities"),
        }
    }

    pub fn completed_sets_today_and_yesterday(&self) -> Result<Vec<SetSummary>> {
        let now = Local::now();
        let today = now.date_naive();
        let start = Local
            .from_local_datetime(
                &(today - Duration::days(1))
                    .and_hms_opt(0, 0, 0)
                    .context("building local start of yesterday")?,
            )
            .earliest()
            .context("determining local start of yesterday")?
            .with_timezone(&Utc);
        let end = Local
            .from_local_datetime(
                &(today + Duration::days(1))
                    .and_hms_opt(0, 0, 0)
                    .context("building local start of tomorrow")?,
            )
            .earliest()
            .context("determining local start of tomorrow")?
            .with_timezone(&Utc);
        let mut stmt = self.conn.prepare(
            r#"
            SELECT s.movement_id, s.muscles_json, s.status, s.reps,
                   COALESCE(r.reps, s.reps), s.weight_kg, s.agent, s.project, s.side, s.created_at
            FROM sets s
            LEFT JOIN recommendations r ON r.id = s.recommendation_id
            WHERE s.status IN ('done', 'skipped', 'pain') AND s.created_at >= ?1 AND s.created_at < ?2
            ORDER BY s.created_at DESC, s.id DESC
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            let muscles_json: String = row.get(1)?;
            Ok(SetSummary {
                movement_id: row.get(0)?,
                muscles: serde_json::from_str(&muscles_json).unwrap_or_default(),
                status: row.get(2)?,
                reps: row.get::<_, i64>(3)? as u32,
                prescribed_reps: row.get::<_, i64>(4)? as u32,
                weight_kg: row.get::<_, Option<f64>>(5)?.map(|value| value as f32),
                agent: row.get(6)?,
                project: row.get(7)?,
                side: side_from_str(row.get(8)?),
                created_at: row.get(9)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading completed exercises from today and yesterday")
    }
}

fn local_period_starts_at(now: DateTime<Local>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let today = now.date_naive();
    let week = today - Duration::days(6);
    let today_start = Local
        .from_local_datetime(
            &today
                .and_hms_opt(0, 0, 0)
                .context("building local start of today")?,
        )
        .earliest()
        .context("determining local start of today")?
        .with_timezone(&Utc);
    let week_start = Local
        .from_local_datetime(
            &week
                .and_hms_opt(0, 0, 0)
                .context("building local start of week")?,
        )
        .earliest()
        .context("determining local start of week")?
        .with_timezone(&Utc);
    Ok((today_start, week_start))
}

fn local_day_bounds_at(now: DateTime<Local>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    day_bounds_in_timezone(now)
}

fn day_bounds_in_timezone<Tz>(now: DateTime<Tz>) -> Result<(DateTime<Utc>, DateTime<Utc>)>
where
    Tz: TimeZone,
{
    let today = now.date_naive();
    let timezone = now.timezone();
    let start = timezone
        .from_local_datetime(
            &today
                .and_hms_opt(0, 0, 0)
                .context("building local start of today")?,
        )
        .earliest()
        .context("determining local start of today")?
        .with_timezone(&Utc);
    let end = timezone
        .from_local_datetime(
            &(today + Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .context("building local start of tomorrow")?,
        )
        .earliest()
        .context("determining local start of tomorrow")?
        .with_timezone(&Utc);
    Ok((start, end))
}

fn fuel_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FuelEntry> {
    let items_json: String = row.get(2)?;
    let parsed = serde_json::from_str(&items_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let created_at: String = row.get(13)?;
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?
        .with_timezone(&Utc);
    Ok(FuelEntry {
        id: row.get(0)?,
        raw_text: row.get(1)?,
        parsed,
        totals: NutritionTotals {
            calories: row.get(3)?,
            protein_g: row.get(4)?,
            carbohydrates_g: row.get(5)?,
            fat_g: row.get(6)?,
            fiber_g: row.get(7)?,
            sugar_g: row.get(8)?,
            sodium_mg: row.get(9)?,
            potassium_mg: row.get(10)?,
        },
        provider: row.get(11)?,
        model: row.get(12)?,
        created_at,
    })
}

fn recommendation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Recommendation> {
    let side: Option<String> = row.get(8)?;
    let agent: String = row.get(9)?;
    let created: String = row.get(11)?;
    Ok(Recommendation {
        id: row.get(0)?,
        movement_id: row.get(1)?,
        movement_name: row.get(2)?,
        primary_muscle: row.get(3)?,
        muscles: serde_json::from_str::<Vec<String>>(&row.get::<_, String>(4)?).unwrap_or_default(),
        reps: row.get::<_, i64>(5)? as u32,
        weight_kg: row.get::<_, Option<f64>>(6)?.map(|value| value as f32),
        estimated_seconds: row.get::<_, i64>(7)? as u32,
        side: side_from_str(side),
        agent: agent.parse().unwrap_or(crate::models::Agent::Custom),
        project: row.get(10)?,
        created_at: DateTime::parse_from_rfc3339(&created)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

fn status_to_str(status: MovementStatus) -> &'static str {
    match status {
        MovementStatus::Allowed => "allowed",
        MovementStatus::Caution => "caution",
        MovementStatus::Blocked => "blocked",
    }
}

fn str_to_status(status: &str) -> MovementStatus {
    match status {
        "allowed" => MovementStatus::Allowed,
        "blocked" => MovementStatus::Blocked,
        _ => MovementStatus::Caution,
    }
}

fn sidedness_to_str(sidedness: MovementSidedness) -> &'static str {
    match sidedness {
        MovementSidedness::Bilateral => "bilateral",
        MovementSidedness::Unilateral => "unilateral",
    }
}

fn sidedness_from_str(sidedness: &str) -> MovementSidedness {
    match sidedness {
        "unilateral" => MovementSidedness::Unilateral,
        _ => MovementSidedness::Bilateral,
    }
}

fn side_to_str(side: RecommendationSide) -> &'static str {
    match side {
        RecommendationSide::Left => "left",
        RecommendationSide::Right => "right",
        RecommendationSide::Bilateral => "bilateral",
    }
}

fn side_from_str(side: Option<String>) -> Option<RecommendationSide> {
    match side.as_deref() {
        Some("left") => Some(RecommendationSide::Left),
        Some("right") => Some(RecommendationSide::Right),
        Some("bilateral") => Some(RecommendationSide::Bilateral),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn database_is_user_only() {
        let root = tempdir().unwrap();
        let database = root.path().join("svarog.sqlite3");

        drop(Store::open(&database).unwrap());

        assert_eq!(
            fs::metadata(database).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn legacy_token_usage_rows_migrate_to_codex_provider() {
        let root = tempdir().unwrap().keep();
        let database = root.join("svarog.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE recommender_token_usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    input_tokens INTEGER NOT NULL,
                    cached_input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    reasoning_output_tokens INTEGER NOT NULL,
                    created_at TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO recommender_token_usage
                    (input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, created_at)
                VALUES (100, 80, 10, 2, ?1)
                "#,
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&database).unwrap();
        let codex = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap();
        let openai = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::OpenAi)
            .unwrap();

        assert_eq!(codex.today.input_tokens, 100);
        assert_eq!(codex.today.output_tokens, 10);
        assert_eq!(openai, RecommenderTokenUsageSummary::default());
    }

    #[test]
    fn old_sessions_schema_migrates_for_codex_lifecycle() {
        let root = tempdir().unwrap().keep();
        let database = root.join("svarog.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent TEXT NOT NULL,
                    project TEXT,
                    created_at TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&database).unwrap();
        let event = CodexHookEvent {
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            cwd: "/work/svarog".into(),
            hook_event_name: "UserPromptSubmit".into(),
            source: None,
            reason: None,
        };

        assert!(store.record_codex_prompt(&event).unwrap());
        assert!(!store.record_codex_prompt(&event).unwrap());
    }

    #[test]
    fn codex_session_end_is_lifecycle_only() {
        let root = tempdir().unwrap().keep();
        let store = Store::open(&root.join("svarog.sqlite3")).unwrap();
        let event = CodexHookEvent {
            session_id: "session-1".into(),
            turn_id: None,
            cwd: "/work/svarog".into(),
            hook_event_name: "SessionStart".into(),
            source: Some("startup".into()),
            reason: None,
        };
        store.record_codex_session(&event).unwrap();
        store.record_codex_session_end(&event).unwrap();

        let ended_at: Option<String> = store
            .conn
            .query_row(
                "SELECT ended_at FROM sessions WHERE external_id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(ended_at.is_some());
        assert!(store.latest_open_recommendation().unwrap().is_none());
    }

    fn store() -> Store {
        let dir = tempdir().unwrap().keep();
        Store::open(&dir.join("svarog.sqlite3")).unwrap()
    }

    fn recommendation() -> Recommendation {
        Recommendation {
            id: None,
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

    fn fuel_parse_with_calories(calories: f64) -> FuelParseResult {
        FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "meal".into(),
                quantity: Some(1.0),
                unit: Some("serving".into()),
                nutrition: NutritionTotals {
                    calories,
                    protein_g: calories / 10.0,
                    carbohydrates_g: calories / 5.0,
                    fat_g: calories / 20.0,
                    fiber_g: calories / 100.0,
                    sugar_g: calories / 25.0,
                    sodium_mg: calories,
                    potassium_mg: calories * 2.0,
                },
                assumptions: Vec::new(),
            }],
        }
    }

    fn event() -> AgentEvent {
        AgentEvent {
            agent: Agent::Codex,
            event: "tool_start".into(),
            expected_duration_sec: 120,
            project: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn recommender_token_usage_sums_today_and_week() {
        let store = store();
        let recent = RecommenderTokenUsage {
            input_tokens: 24_763,
            cached_input_tokens: 24_448,
            output_tokens: 122,
            reasoning_output_tokens: 7,
        };
        let old = RecommenderTokenUsage {
            input_tokens: 10_000,
            output_tokens: 50,
            ..RecommenderTokenUsage::default()
        };
        store
            .record_recommender_token_usage(RecommenderTokenProvider::Codex, &recent, Utc::now())
            .unwrap();
        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::Codex,
                &old,
                Utc::now() - Duration::days(8),
            )
            .unwrap();
        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::OpenAi,
                &RecommenderTokenUsage {
                    input_tokens: 500,
                    output_tokens: 25,
                    ..RecommenderTokenUsage::default()
                },
                Utc::now(),
            )
            .unwrap();

        let summary = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
            .unwrap();

        assert_eq!(summary.today.input_tokens, 24_763);
        assert_eq!(summary.today.output_tokens, 122);
        assert_eq!(summary.week.input_tokens, 24_763);
        assert_eq!(summary.week.output_tokens, 122);
        let openai = store
            .recommender_token_usage_summary_for(RecommenderTokenProvider::OpenAi)
            .unwrap();
        assert_eq!(openai.today.input_tokens, 500);
        assert_eq!(openai.today.output_tokens, 25);
    }

    #[test]
    fn rolling_week_includes_sunday_usage_and_forges_on_monday() {
        let store = store();
        let monday = Local
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .unwrap();
        let sunday = (monday - Duration::days(1)).with_timezone(&Utc);
        let previous_monday = (monday - Duration::days(7)).with_timezone(&Utc);

        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::Codex,
                &RecommenderTokenUsage {
                    input_tokens: 100,
                    output_tokens: 10,
                    ..RecommenderTokenUsage::default()
                },
                sunday,
            )
            .unwrap();
        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::Codex,
                &RecommenderTokenUsage {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    ..RecommenderTokenUsage::default()
                },
                previous_monday,
            )
            .unwrap();
        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::OpenAi,
                &RecommenderTokenUsage {
                    input_tokens: 500,
                    output_tokens: 25,
                    ..RecommenderTokenUsage::default()
                },
                monday.with_timezone(&Utc),
            )
            .unwrap();

        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());
        store
            .record_set_with_reps(&rec, SetStatus::Done, 15)
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE sets SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM sets)",
                [sunday.to_rfc3339()],
            )
            .unwrap();
        store
            .record_set_with_reps(&rec, SetStatus::Done, 20)
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE sets SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM sets)",
                [previous_monday.to_rfc3339()],
            )
            .unwrap();

        let codex = store
            .recommender_token_usage_summary_for_at(RecommenderTokenProvider::Codex, monday)
            .unwrap();
        assert_eq!(codex.today, TokenUsageTotals::default());
        assert_eq!(
            codex.week,
            TokenUsageTotals {
                input_tokens: 100,
                output_tokens: 10,
            }
        );

        let openai = store
            .recommender_token_usage_summary_for_at(RecommenderTokenProvider::OpenAi, monday)
            .unwrap();
        assert_eq!(
            openai.today,
            TokenUsageTotals {
                input_tokens: 500,
                output_tokens: 25,
            }
        );
        assert_eq!(openai.week, openai.today);

        let activity = store.completed_forge_summary_at(monday).unwrap();
        assert_eq!(activity.today, ForgeActivityTotals::default());
        assert_eq!(
            activity.week,
            ForgeActivityTotals {
                forges: 1,
                reps: 15,
            }
        );
    }

    #[test]
    fn full_reset_clears_recommender_token_usage() {
        let store = store();
        store
            .record_recommender_token_usage(
                RecommenderTokenProvider::Codex,
                &RecommenderTokenUsage {
                    input_tokens: 100,
                    output_tokens: 10,
                    ..RecommenderTokenUsage::default()
                },
                Utc::now(),
            )
            .unwrap();

        store.reset_all_data().unwrap();

        assert_eq!(
            store
                .recommender_token_usage_summary_for(RecommenderTokenProvider::Codex)
                .unwrap(),
            RecommenderTokenUsageSummary::default()
        );
        assert_eq!(
            store.completed_forge_summary().unwrap(),
            ForgeActivitySummary::default()
        );
    }

    #[test]
    fn completed_forge_summary_counts_only_recent_done_sets_and_actual_reps() {
        let store = store();
        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());

        store
            .record_set_with_reps(&rec, SetStatus::Done, 15)
            .unwrap();
        store
            .record_set_with_reps(&rec, SetStatus::Skipped, 99)
            .unwrap();
        store
            .record_set_with_reps(&rec, SetStatus::Pain, 99)
            .unwrap();
        store
            .record_set_with_reps(&rec, SetStatus::Started, 99)
            .unwrap();
        store
            .record_set_with_reps(&rec, SetStatus::Done, 20)
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE sets SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM sets)",
                [Utc::now()
                    .checked_sub_signed(Duration::days(8))
                    .unwrap()
                    .to_rfc3339()],
            )
            .unwrap();

        let summary = store.completed_forge_summary().unwrap();

        assert_eq!(
            summary.today,
            ForgeActivityTotals {
                forges: 1,
                reps: 15,
            }
        );
        assert_eq!(summary.week, summary.today);
    }

    #[test]
    fn recent_forge_history_returns_newest_ten_recorded_outcomes() {
        let store = store();
        let now = Utc::now();
        for index in 0..12 {
            let mut rec = recommendation();
            rec.movement_name = format!("forge {index}");
            rec.id = Some(store.insert_recommendation(&rec).unwrap());
            let status = match index % 3 {
                0 => SetStatus::Done,
                1 => SetStatus::Skipped,
                _ => SetStatus::Pain,
            };
            store.record_set_with_reps(&rec, status, index + 1).unwrap();
            store
                .conn
                .execute(
                    "UPDATE sets SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM sets)",
                    [(now - Duration::minutes(i64::from(index))).to_rfc3339()],
                )
                .unwrap();
        }
        let mut started = recommendation();
        started.movement_name = "unanswered forge".to_string();
        started.id = Some(store.insert_recommendation(&started).unwrap());
        store.record_set(&started, SetStatus::Started).unwrap();

        let history = store.recent_forge_history(10).unwrap();

        assert_eq!(history.len(), 10);
        assert_eq!(history.first().unwrap().movement_name, "forge 0");
        assert_eq!(history.first().unwrap().status, "done");
        assert_eq!(history.first().unwrap().reps, 1);
        assert_eq!(history.last().unwrap().movement_name, "forge 9");
        assert!(history
            .iter()
            .all(|entry| entry.movement_name != "unanswered forge"));
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.status.as_str())
                .take(3)
                .collect::<Vec<_>>(),
            vec!["done", "skipped", "pain"]
        );
    }

    #[test]
    fn record_set_with_reps_stores_actual_reps() {
        let store = store();
        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());

        store
            .record_set_with_reps(&rec, SetStatus::Done, 15)
            .unwrap();

        let (_, reps, _) = store.stats_today().unwrap();
        assert_eq!(reps, 15);
    }

    #[test]
    fn daily_forge_count_is_not_limited_by_total_repetitions() {
        let store = store();
        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());
        for reps in [7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6] {
            store
                .record_set_with_reps(&rec, SetStatus::Done, reps)
                .unwrap();
        }

        assert_eq!(store.today_set_count().unwrap(), 16);
        assert_eq!(store.stats_today().unwrap(), (16, 106, 0));
    }

    #[test]
    fn today_counts_use_local_calendar_day_boundaries() {
        let store = store();
        let now = Local::now();
        let (start, end) = local_day_bounds_at(now).unwrap();
        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());

        for (offset, reps) in [
            (Duration::seconds(-1), 1),
            (Duration::seconds(0), 2),
            (end - start - Duration::seconds(1), 3),
            (end - start, 4),
        ] {
            store
                .record_set_with_reps(&rec, SetStatus::Done, reps)
                .unwrap();
            store
                .conn
                .execute(
                    "UPDATE sets SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM sets)",
                    [(start + offset).to_rfc3339()],
                )
                .unwrap();
        }

        assert_eq!(store.today_set_count().unwrap(), 2);
        assert_eq!(store.stats_today().unwrap(), (2, 5, 0));
    }

    #[test]
    fn profile_refresh_does_not_unblock_movement_with_pain_history() {
        let store = store();
        store.seed_movements().unwrap();
        let mut rec = recommendation();
        rec.movement_id = "Bodyweight_Squat".into();
        rec.movement_name = "Bodyweight Squat".into();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());
        store.record_set(&rec, SetStatus::Pain).unwrap();
        let mut movement = store
            .movements()
            .unwrap()
            .into_iter()
            .find(|movement| movement.id == "Bodyweight_Squat")
            .unwrap();
        movement.status = MovementStatus::Allowed;

        store.upsert_movement(&movement).unwrap();

        let refreshed = store
            .movements()
            .unwrap()
            .into_iter()
            .find(|movement| movement.id == "Bodyweight_Squat")
            .unwrap();
        assert_eq!(refreshed.status, MovementStatus::Blocked);
    }

    #[test]
    fn fatigue_suppression_covers_next_five_events() {
        let store = store();
        store.insert_event(&event()).unwrap();
        store.suppress_next_opportunities(5).unwrap();

        for _ in 0..5 {
            store.insert_event(&event()).unwrap();
            assert!(store.fatigue_suppression_active().unwrap());
        }

        store.insert_event(&event()).unwrap();
        assert!(!store.fatigue_suppression_active().unwrap());
        assert!(store.state().unwrap().suppress_until_event_count.is_none());
    }

    #[test]
    fn reset_all_data_clears_history_and_restores_idle_state() {
        let store = store();
        let mut rec = recommendation();
        rec.id = Some(store.insert_recommendation(&rec).unwrap());
        store.insert_session(Agent::Codex, Some("svarog")).unwrap();
        store.insert_event(&event()).unwrap();
        store.record_set(&rec, SetStatus::Done).unwrap();
        store.save_user_profile(&Config::default()).unwrap();
        store.seed_movements().unwrap();
        store
            .save_fuel_entry(
                "test meal",
                &FuelParseResult {
                    items: vec![crate::models::FuelItem {
                        name: "test meal".into(),
                        quantity: None,
                        unit: None,
                        nutrition: NutritionTotals {
                            calories: 100.0,
                            carbohydrates_g: 10.0,
                            ..NutritionTotals::default()
                        },
                        assumptions: Vec::new(),
                    }],
                },
                "codex",
                "gpt-5.6-luna",
                Utc::now(),
            )
            .unwrap();
        store.adjust_water_today(200.0, UnitSystem::Metric).unwrap();

        store.reset_all_data().unwrap();

        let (sets, reps, breaks) = store.stats_today().unwrap();
        assert_eq!((sets, reps, breaks), (0, 0, 0));
        assert!(store.movements().unwrap().is_empty());
        assert!(store.latest_open_recommendation().unwrap().is_none());
        assert!(store.recent_fuel_entries(5).unwrap().is_empty());
        assert_eq!(store.water_total_today().unwrap(), WaterTotal::default());
        assert_eq!(store.state().unwrap().kind, AppStateKind::Idle);
    }

    #[test]
    fn reset_all_data_removes_deleted_content_from_database_files() {
        let root = tempdir().unwrap().keep();
        let database = root.join("svarog.sqlite3");
        let store = Store::open(&database).unwrap();
        let marker = "SVAROG_RESET_FORENSIC_MARKER_9f6b58d9";
        let mut sensitive_event = event();
        sensitive_event.project = Some(marker.into());
        store.insert_event(&sensitive_event).unwrap();

        store.reset_all_data().unwrap();

        for path in [
            database.clone(),
            database.with_extension("sqlite3-wal"),
            database.with_extension("sqlite3-shm"),
        ] {
            if path.exists() {
                let bytes = fs::read(&path).unwrap();
                assert!(
                    !bytes
                        .windows(marker.len())
                        .any(|window| window == marker.as_bytes()),
                    "reset marker remained in {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn settings_profile_and_movement_pool_update_together() {
        let store = store();
        let mut config = Config::default();
        config.profile.goals = vec!["mobility".into()];
        let movements = crate::exercise_catalog::movements_for_equipment(&["bodyweight".into()]);
        let equipment_filter = r#"[{"kind":"bodyweight","weights_kg":[],"count":1}]"#;

        store
            .apply_user_profile_and_movement_pool(&config, None, &movements, equipment_filter)
            .unwrap();

        let profile_json: String = store
            .conn
            .query_row("SELECT profile_json FROM users WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(profile_json.contains("mobility"));
        assert_eq!(store.movements().unwrap().len(), movements.len());
        let saved_filter: serde_json::Value = store.exercise_filter().unwrap().unwrap();
        assert_eq!(saved_filter[0]["count"], 1);
    }

    #[test]
    fn settings_pool_update_keeps_compatible_queue_and_retires_incompatible_items() {
        let store = store();
        let config = Config::default();
        let movements = crate::exercise_catalog::movements_for_equipment(&["bodyweight".into()]);
        let compatible_movement = &movements[0];
        let mut compatible = recommendation();
        compatible.movement_id = compatible_movement.id.clone();
        compatible.movement_name = compatible_movement.name.clone();
        compatible.primary_muscle = compatible_movement.primary_muscle.clone();
        compatible.muscles = compatible_movement.muscles.clone();
        store.insert_queued_recommendation(&compatible).unwrap();
        store
            .insert_queued_recommendation(&recommendation())
            .unwrap();

        store
            .apply_user_profile_and_movement_pool(&config, None, &movements, "[]")
            .unwrap();

        let queued = store.queued_recommendations().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].movement_id, compatible.movement_id);
    }

    #[test]
    fn queued_recommendations_do_not_become_visible_until_promoted() {
        let store = store();
        let rec = recommendation();
        let mut second = recommendation();
        second.movement_name = "second forge".to_string();

        store.insert_queued_recommendation(&rec).unwrap();
        store.insert_queued_recommendation(&second).unwrap();

        assert_eq!(store.queued_recommendation_count().unwrap(), 2);
        assert_eq!(
            store
                .queued_recommendations()
                .unwrap()
                .into_iter()
                .map(|rec| rec.movement_name)
                .collect::<Vec<_>>(),
            vec!["left curl", "second forge"]
        );
        assert!(store.latest_open_recommendation().unwrap().is_none());
        assert_eq!(store.state().unwrap().kind, AppStateKind::Idle);
    }

    #[test]
    fn clearing_queue_preserves_current_forge_and_history() {
        let store = store();
        let mut current = recommendation();
        current.id = Some(store.insert_recommendation(&current).unwrap());
        let mut queued = recommendation();
        queued.movement_id = "queued".into();
        store.insert_queued_recommendation(&queued).unwrap();
        store.insert_event(&event()).unwrap();

        store.clear_queued_recommendations().unwrap();

        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
        assert_eq!(
            store.latest_open_recommendation().unwrap().unwrap().id,
            current.id
        );
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
    }

    #[test]
    fn replacing_queue_is_ordered_and_preserves_nonqueued_data() {
        let mut store = store();
        let mut current = recommendation();
        current.id = Some(store.insert_recommendation(&current).unwrap());
        store.record_set(&current, SetStatus::Done).unwrap();
        store.insert_event(&event()).unwrap();
        let mut old = recommendation();
        old.movement_name = "old queued forge".into();
        store.insert_queued_recommendation(&old).unwrap();
        let mut first = recommendation();
        first.movement_name = "new first".into();
        let mut second = recommendation();
        second.movement_name = "new second".into();

        store
            .replace_queued_recommendations(&[first, second])
            .unwrap();

        assert_eq!(
            store
                .queued_recommendations()
                .unwrap()
                .into_iter()
                .map(|rec| rec.movement_name)
                .collect::<Vec<_>>(),
            vec!["new first", "new second"]
        );
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.recent_forge_history(10).unwrap().len(), 1);
    }

    #[test]
    fn promoting_queued_recommendation_sets_current_forge() {
        let store = store();
        let rec = recommendation();
        store.insert_queued_recommendation(&rec).unwrap();

        let promoted = store
            .promote_next_queued_recommendation(Agent::Codex, Some("svarog"))
            .unwrap()
            .unwrap();

        assert_eq!(promoted.agent, Agent::Codex);
        assert_eq!(promoted.project.as_deref(), Some("svarog"));
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
        assert!(store.latest_open_recommendation().unwrap().is_some());
    }

    #[test]
    fn manually_promoting_queued_recommendation_preserves_metadata() {
        let store = store();
        let mut rec = recommendation();
        rec.agent = Agent::Claude;
        rec.project = Some("manual-project".into());
        store.insert_queued_recommendation(&rec).unwrap();

        let promoted = store
            .promote_next_queued_recommendation_preserving_metadata()
            .unwrap()
            .unwrap();

        assert_eq!(promoted.agent, Agent::Claude);
        assert_eq!(promoted.project.as_deref(), Some("manual-project"));
        assert_eq!(store.state().unwrap().kind, AppStateKind::Recommendation);
    }

    #[test]
    fn queued_promotion_skips_unrecovered_muscle_without_discarding_it() {
        let store = store();
        let mut completed = recommendation();
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store
            .mark_recommendation(completed.id.unwrap(), "done")
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        let cooling = recommendation();
        store.insert_queued_recommendation(&cooling).unwrap();
        let mut recovered = recommendation();
        recovered.movement_id = "scapular_squeeze".into();
        recovered.movement_name = "scapular squeezes".into();
        recovered.primary_muscle = "upper_back".into();
        recovered.muscles = vec!["upper_back".into(), "shoulders".into()];
        store.insert_queued_recommendation(&recovered).unwrap();

        let promoted = store
            .promote_next_queued_recommendation(Agent::Codex, Some("svarog"))
            .unwrap()
            .unwrap();

        assert_eq!(promoted.primary_muscle, "upper_back");
        assert_eq!(store.queued_recommendation_count().unwrap(), 1);
    }

    #[test]
    fn queued_promotion_keeps_cooldown_when_no_muscle_is_recovered() {
        let store = store();
        let mut completed = recommendation();
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store
            .mark_recommendation(completed.id.unwrap(), "done")
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();
        store
            .insert_queued_recommendation(&recommendation())
            .unwrap();

        let promoted = store
            .promote_next_queued_recommendation(Agent::Codex, Some("svarog"))
            .unwrap();

        assert!(promoted.is_none());
        assert_eq!(store.queued_recommendation_count().unwrap(), 1);
        assert_eq!(store.state().unwrap().kind, AppStateKind::Cooldown);
    }

    #[test]
    fn queued_promotion_allows_the_opposite_side_of_a_cooling_muscle() {
        let store = store();
        let mut completed = recommendation();
        completed.side = Some(RecommendationSide::Right);
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store
            .mark_recommendation(completed.id.unwrap(), "done")
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        let mut queued = recommendation();
        queued.side = Some(RecommendationSide::Left);
        store.insert_queued_recommendation(&queued).unwrap();

        let promoted = store
            .promote_next_queued_recommendation(Agent::Codex, Some("svarog"))
            .unwrap()
            .unwrap();

        assert_eq!(promoted.side, Some(RecommendationSide::Left));
        assert_eq!(store.queued_recommendation_count().unwrap(), 0);
    }

    #[test]
    fn queued_promotion_still_blocks_the_same_side_of_a_cooling_muscle() {
        let store = store();
        let mut completed = recommendation();
        completed.side = Some(RecommendationSide::Right);
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store
            .mark_recommendation(completed.id.unwrap(), "done")
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        let mut queued = recommendation();
        queued.side = Some(RecommendationSide::Right);
        store.insert_queued_recommendation(&queued).unwrap();

        assert!(store
            .promote_next_queued_recommendation(Agent::Codex, Some("svarog"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn bilateral_and_legacy_sets_block_both_sides() {
        for completed_side in [Some(RecommendationSide::Bilateral), None] {
            let store = store();
            let mut completed = recommendation();
            completed.side = completed_side;
            completed.id = Some(store.insert_recommendation(&completed).unwrap());
            store
                .mark_recommendation(completed.id.unwrap(), "done")
                .unwrap();
            store.record_set(&completed, SetStatus::Done).unwrap();

            for candidate_side in [RecommendationSide::Left, RecommendationSide::Right] {
                assert!(!store
                    .muscle_side_recovered(
                        &completed.primary_muscle,
                        Some(candidate_side),
                        MUSCLE_COOLDOWN_MINUTES,
                    )
                    .unwrap());
            }
        }
    }

    #[test]
    fn bilateral_candidate_is_blocked_by_either_unilateral_side() {
        let store = store();
        let mut completed = recommendation();
        completed.side = Some(RecommendationSide::Left);
        completed.id = Some(store.insert_recommendation(&completed).unwrap());
        store
            .mark_recommendation(completed.id.unwrap(), "done")
            .unwrap();
        store.record_set(&completed, SetStatus::Done).unwrap();

        assert!(!store
            .muscle_side_recovered(
                &completed.primary_muscle,
                Some(RecommendationSide::Bilateral),
                MUSCLE_COOLDOWN_MINUTES,
            )
            .unwrap());
        assert!(!store
            .muscle_recovered(&completed.primary_muscle, MUSCLE_COOLDOWN_MINUTES)
            .unwrap());
    }

    #[test]
    fn excluding_an_exercise_hides_it_and_retires_queued_copies() {
        let store = store();
        store.seed_movements().unwrap();
        let movement = store
            .movements()
            .unwrap()
            .into_iter()
            .find(|movement| movement.id == "Dead_Bug")
            .unwrap();
        let mut rec = recommendation();
        rec.movement_id = movement.id.clone();
        rec.movement_name = movement.name;
        rec.primary_muscle = movement.primary_muscle;
        rec.muscles = movement.muscles;
        store.insert_queued_recommendation(&rec).unwrap();

        store.exclude_exercise("Dead_Bug").unwrap();

        assert!(!store
            .movements()
            .unwrap()
            .iter()
            .any(|movement| movement.id == "Dead_Bug"));
        assert!(store.queued_recommendations().unwrap().is_empty());
        assert_eq!(store.removed_exercise_ids().unwrap(), vec!["Dead_Bug"]);
        assert!(store.restore_exercise("Dead_Bug").unwrap());
        assert!(store
            .movements()
            .unwrap()
            .iter()
            .any(|movement| movement.id == "Dead_Bug"));
    }

    #[test]
    fn canonical_pool_migration_retires_open_legacy_items_but_keeps_history() {
        let store = store();
        let mut legacy = recommendation();
        legacy.id = Some(store.insert_recommendation(&legacy).unwrap());
        store.record_set(&legacy, SetStatus::Skipped).unwrap();
        store
            .mark_recommendation(legacy.id.unwrap(), "active")
            .unwrap();

        let equipment = vec!["bodyweight".to_string()];
        store
            .replace_movement_pool(&crate::exercise_catalog::movements_for_equipment(
                &equipment,
            ))
            .unwrap();

        assert!(store.latest_open_recommendation().unwrap().is_none());
        assert_eq!(store.recent_forge_history(10).unwrap().len(), 1);
    }

    #[test]
    fn fuel_entries_round_trip_and_delete() {
        let store = store();
        let parsed = FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "oatmeal".into(),
                quantity: Some(250.0),
                unit: Some("g".into()),
                nutrition: NutritionTotals {
                    calories: 320.0,
                    protein_g: 12.0,
                    carbohydrates_g: 54.0,
                    fat_g: 7.0,
                    fiber_g: 8.0,
                    sugar_g: 10.0,
                    sodium_mg: 120.0,
                    potassium_mg: 410.0,
                },
                assumptions: vec!["ordinary cooked oatmeal".into()],
            }],
        };
        let id = store
            .save_fuel_entry(
                "oatmeal with milk",
                &parsed,
                "codex",
                "gpt-5.6-luna",
                Utc::now(),
            )
            .unwrap();

        let recent = store.recent_fuel_entries(5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, id);
        assert_eq!(recent[0].parsed, parsed);
        assert_eq!(recent[0].totals.calories, 320.0);
        assert!(store.delete_fuel_entry(id).unwrap());
        assert!(store.recent_fuel_entries(5).unwrap().is_empty());
    }

    #[test]
    fn recent_fuel_entries_return_the_latest_five_across_days() {
        let store = store();
        let now = Utc::now();
        let parsed = FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "meal".into(),
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
        for index in (0..7).rev() {
            store
                .save_fuel_entry(
                    &format!("meal {index}"),
                    &parsed,
                    "codex",
                    "gpt-5.6-luna",
                    now - Duration::days(index),
                )
                .unwrap();
        }

        let recent = store.recent_fuel_entries(5).unwrap();
        assert_eq!(recent.len(), 5);
        assert_eq!(
            recent
                .iter()
                .map(|entry| entry.raw_text.as_str())
                .collect::<Vec<_>>(),
            vec!["meal 0", "meal 1", "meal 2", "meal 3", "meal 4"]
        );
    }

    #[test]
    fn fuel_batch_saves_atomically_in_consumption_order() {
        let store = store();
        let now = Utc::now();
        let parsed = FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "meal".into(),
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
        let events = vec![
            TimedFuelEvent {
                consumed_at: now - Duration::hours(4),
                source_text: "breakfast".into(),
                parsed: parsed.clone(),
            },
            TimedFuelEvent {
                consumed_at: now - Duration::hours(1),
                source_text: "lunch".into(),
                parsed: parsed.clone(),
            },
        ];

        let ids = store
            .save_fuel_batch(&events, "codex", "gpt-5.6-luna")
            .unwrap();
        assert_eq!(ids.len(), 2);
        let recent = store.recent_fuel_entries(5).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].created_at, events[1].consumed_at);
        assert_eq!(recent[1].created_at, events[0].consumed_at);
        assert_eq!(recent[0].raw_text, "lunch");
        assert_eq!(recent[1].raw_text, "breakfast");

        let mut invalid = events.clone();
        invalid[1].parsed.items[0].name = " ".into();
        assert!(store
            .save_fuel_batch(&invalid, "codex", "gpt-5.6-luna")
            .is_err());
        assert_eq!(store.recent_fuel_entries(5).unwrap().len(), 2);
    }

    #[test]
    fn fuel_batch_preserves_and_sums_equal_time_repeated_foods() {
        let store = store();
        let consumed_at = Utc::now();
        let parsed = FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "milk".into(),
                quantity: Some(100.0),
                unit: Some("ml".into()),
                nutrition: NutritionTotals {
                    calories: 60.0,
                    protein_g: 3.0,
                    ..NutritionTotals::default()
                },
                assumptions: Vec::new(),
            }],
        };
        let events = vec![
            TimedFuelEvent {
                consumed_at,
                source_text: "milk in coffee".into(),
                parsed: parsed.clone(),
            },
            TimedFuelEvent {
                consumed_at,
                source_text: "milk in protein shake".into(),
                parsed,
            },
        ];

        store
            .save_fuel_batch(&events, "openai", "gpt-5.6-luna")
            .unwrap();

        let recent = store.recent_fuel_entries(5).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].created_at, recent[1].created_at);
        let totals = store.nutrition_totals_today().unwrap();
        assert_eq!(totals.calories, 120.0);
        assert_eq!(totals.protein_g, 6.0);
    }

    #[test]
    fn nutrition_totals_sum_only_the_current_local_day() {
        let store = store();
        let now = Local::now();
        assert_eq!(
            store.nutrition_totals_today_at(now).unwrap(),
            NutritionTotals::default()
        );
        let parsed = FuelParseResult {
            items: vec![crate::models::FuelItem {
                name: "meal".into(),
                quantity: Some(1.0),
                unit: Some("serving".into()),
                nutrition: NutritionTotals {
                    calories: 320.0,
                    protein_g: 12.0,
                    carbohydrates_g: 54.0,
                    fat_g: 7.0,
                    fiber_g: 8.0,
                    sugar_g: 10.0,
                    sodium_mg: 120.0,
                    potassium_mg: 410.0,
                },
                assumptions: Vec::new(),
            }],
        };
        for created_at in [now, now, now - Duration::days(2)] {
            store
                .save_fuel_entry(
                    "meal",
                    &parsed,
                    "codex",
                    "gpt-5.6-luna",
                    created_at.with_timezone(&Utc),
                )
                .unwrap();
        }

        let totals = store.nutrition_totals_today_at(now).unwrap();
        assert_eq!(totals.calories, 640.0);
        assert_eq!(totals.protein_g, 24.0);
        assert_eq!(totals.carbohydrates_g, 108.0);
        assert_eq!(totals.fat_g, 14.0);
        assert_eq!(totals.fiber_g, 16.0);
        assert_eq!(totals.sugar_g, 20.0);
        assert_eq!(totals.sodium_mg, 240.0);
        assert_eq!(totals.potassium_mg, 820.0);
    }

    #[test]
    fn nutrition_average_uses_the_latest_seven_logged_local_days() {
        let store = store();
        let now = Local::now();
        assert!(store
            .nutrition_average_recent_logged_days_at(now)
            .unwrap()
            .is_none());

        for day_offset in 0..8 {
            let date = now.date_naive() - Duration::days(day_offset);
            let noon = Local
                .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
                .earliest()
                .unwrap();
            let base_time = if day_offset == 0 {
                now - Duration::minutes(2)
            } else {
                noon
            };
            let daily_calories = (day_offset + 1) as f64 * 70.0;
            let portions = if day_offset == 0 {
                vec![20.0, daily_calories - 20.0]
            } else {
                vec![daily_calories]
            };
            for (index, calories) in portions.into_iter().enumerate() {
                store
                    .save_fuel_entry(
                        "meal",
                        &fuel_parse_with_calories(calories),
                        "codex",
                        "gpt-5.6-luna",
                        (base_time + Duration::minutes(index as i64)).with_timezone(&Utc),
                    )
                    .unwrap();
            }
        }

        let average = store
            .nutrition_average_recent_logged_days_at(now)
            .unwrap()
            .unwrap();
        assert_eq!(average.logged_days, 7);
        assert_eq!(average.totals.calories, 280.0);
        assert_eq!(average.totals.protein_g, 28.0);

        let partial_root = tempdir().unwrap().keep();
        let partial_store = Store::open(&partial_root.join("svarog.sqlite3")).unwrap();
        for day_offset in 0..3 {
            let date = now.date_naive() - Duration::days(day_offset);
            let noon = Local
                .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
                .earliest()
                .unwrap();
            let created_at = if day_offset == 0 {
                now - Duration::minutes(1)
            } else {
                noon
            };
            partial_store
                .save_fuel_entry(
                    "meal",
                    &fuel_parse_with_calories((day_offset + 1) as f64 * 100.0),
                    "codex",
                    "gpt-5.6-luna",
                    created_at.with_timezone(&Utc),
                )
                .unwrap();
        }
        let partial_average = partial_store
            .nutrition_average_recent_logged_days_at(now)
            .unwrap()
            .unwrap();
        assert_eq!(partial_average.logged_days, 3);
        assert_eq!(partial_average.totals.calories, 200.0);
    }

    #[test]
    fn weight_checkins_preserve_the_first_weight_and_record_applied_changes() {
        let store = store();
        let movements = crate::exercise_catalog::movements_for_equipment(&["bodyweight".into()]);
        let mut current = Config::default();
        current.profile.weight_kg = Some(77.0);

        store
            .apply_user_profile_and_movement_pool(&current, Some(80.0), &movements, "[]")
            .unwrap();
        assert_eq!(
            store.weight_progress().unwrap(),
            Some(WeightProgress {
                starting_kg: 80.0,
                current_kg: 77.0,
            })
        );

        store
            .apply_user_profile_and_movement_pool(&current, Some(77.0), &movements, "[]")
            .unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM weight_checkins", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn water_adjustments_store_both_units_clamp_and_roll_over_locally() {
        let store = store();
        let now = Local::now();
        let first = store
            .adjust_water_at(200.0, UnitSystem::Metric, now)
            .unwrap();
        assert_eq!(first.milliliters, 200.0);
        assert!((first.fluid_ounces - 200.0 / ML_PER_US_FL_OZ).abs() < 0.000_001);

        let imperial_step = 8.0 * ML_PER_US_FL_OZ;
        let second = store
            .adjust_water_at(imperial_step, UnitSystem::Imperial, now)
            .unwrap();
        assert!((second.fluid_ounces - (first.fluid_ounces + 8.0)).abs() < 0.000_001);
        let stored: (f64, f64) = store
            .conn
            .query_row(
                "SELECT total_ml, total_fl_oz FROM water_adjustments ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((stored.0 / ML_PER_US_FL_OZ - stored.1).abs() < 0.000_001);

        let zero = store
            .adjust_water_at(-10_000.0, UnitSystem::Metric, now)
            .unwrap();
        assert_eq!(zero, WaterTotal::default());
        assert_eq!(store.water_total_at(now).unwrap(), WaterTotal::default());
        assert_eq!(
            store.water_total_at(now + Duration::days(1)).unwrap(),
            WaterTotal::default()
        );
    }

    #[test]
    fn local_day_bounds_follow_dst_calendar_midnights() {
        let spring = chrono_tz::America::New_York
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .single()
            .unwrap();
        let fall = chrono_tz::America::New_York
            .with_ymd_and_hms(2026, 11, 1, 12, 0, 0)
            .single()
            .unwrap();

        let (spring_start, spring_end) = day_bounds_in_timezone(spring).unwrap();
        let (fall_start, fall_end) = day_bounds_in_timezone(fall).unwrap();

        assert_eq!((spring_end - spring_start).num_hours(), 23);
        assert_eq!((fall_end - fall_start).num_hours(), 25);
    }

    #[test]
    fn concurrent_water_adjustments_serialize() {
        let root = tempdir().unwrap().keep();
        let database = root.join("svarog.sqlite3");
        drop(Store::open(&database).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let database = database.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let store = Store::open(&database).unwrap();
                barrier.wait();
                store.adjust_water_today(200.0, UnitSystem::Metric).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        let store = Store::open(&database).unwrap();
        assert_eq!(store.water_total_today().unwrap().milliliters, 400.0);
    }
}
