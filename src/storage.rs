use crate::config::Config;
use crate::models::{
    Agent, AgentEvent, AppState, AppStateKind, CodexHookEvent, ForgeActivitySummary,
    ForgeActivityTotals, Movement, MovementSidedness, MovementStatus, Recommendation,
    RecommendationSide, RecommenderTokenProvider, RecommenderTokenUsage,
    RecommenderTokenUsageSummary, SetStatus, TokenUsageTotals,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const MUSCLE_COOLDOWN_MINUTES: i64 = 18;

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
        self.conn.execute_batch(
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
                'recommender_token_usage'
            );
            "#,
        )?;
        self.conn.execute(
            r#"
            INSERT INTO app_state
                (id, kind, current_recommendation_id, cooldown_muscle, cooldown_until, suppress_until_event_count, updated_at)
            VALUES (1, 'idle', NULL, NULL, NULL, NULL, ?1)
            "#,
            [Utc::now().to_rfc3339()],
        )?;
        Ok(())
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
        transaction
            .commit()
            .context("committing exercise-pool replacement")
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
            "SELECT id, primary_muscle FROM recommendations WHERE status = 'queued' ORDER BY id ASC",
        )?;
        let candidates = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let mut recovered_id = None;
        for (id, primary_muscle) in candidates {
            if self.muscle_recovered(&primary_muscle, MUSCLE_COOLDOWN_MINUTES)? {
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
        let last_done = self
            .conn
            .query_row(
                r#"
                SELECT s.created_at
                FROM sets s
                JOIN recommendations r ON r.id = s.recommendation_id
                WHERE s.status = 'done' AND r.primary_muscle = ?1
                ORDER BY s.id DESC
                LIMIT 1
                "#,
                [muscle],
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
        let today = Utc::now().date_naive().to_string();
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM sets WHERE substr(created_at, 1, 10) = ?1 AND status = 'done'",
                [today],
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
        let today = Utc::now().date_naive().to_string();
        self.conn
            .query_row(
                r#"
                SELECT
                    SUM(CASE WHEN status = 'done' THEN 1 ELSE 0 END),
                    COALESCE(SUM(CASE WHEN status = 'done' THEN reps ELSE 0 END), 0),
                    SUM(CASE WHEN status IN ('skipped', 'pain') THEN 1 ELSE 0 END)
                FROM sets
                WHERE substr(created_at, 1, 10) = ?1
                "#,
                [today],
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

    pub fn recent_outcomes(&self, limit: u32) -> Result<Vec<OutcomeSummary>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT s.movement_id, s.status, COALESCE(r.reps, s.reps), s.reps, s.created_at
            FROM sets s
            LEFT JOIN recommendations r ON r.id = s.recommendation_id
            WHERE s.status IN ('done', 'skipped', 'pain')
            ORDER BY s.created_at DESC, s.id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = statement.query_map([i64::from(limit)], |row| {
            Ok(OutcomeSummary {
                movement_id: row.get(0)?,
                status: row.get(1)?,
                prescribed_reps: row.get::<_, i64>(2)? as u32,
                actual_reps: row.get::<_, i64>(3)? as u32,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("loading recent forge outcomes")
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

        store.reset_all_data().unwrap();

        let (sets, reps, breaks) = store.stats_today().unwrap();
        assert_eq!((sets, reps, breaks), (0, 0, 0));
        assert!(store.movements().unwrap().is_empty());
        assert!(store.latest_open_recommendation().unwrap().is_none());
        assert_eq!(store.state().unwrap().kind, AppStateKind::Idle);
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
}
