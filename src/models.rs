use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Codex,
    Claude,
    Droid,
    #[value(alias = "factory_droid")]
    #[serde(alias = "factory-droid")]
    FactoryDroid,
    #[value(alias = "openclaw")]
    #[serde(alias = "open-claw")]
    OpenClaw,
    #[value(alias = "generic")]
    #[serde(alias = "generic")]
    Custom,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
            Agent::Droid => "droid",
            Agent::FactoryDroid => "factory_droid",
            Agent::OpenClaw => "openclaw",
            Agent::Custom => "custom",
        }
    }
}

impl std::str::FromStr for Agent {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Agent::Codex),
            "claude" => Ok(Agent::Claude),
            "droid" => Ok(Agent::Droid),
            "factory-droid" | "factory_droid" => Ok(Agent::FactoryDroid),
            "openclaw" | "open-claw" => Ok(Agent::OpenClaw),
            "custom" | "generic" => Ok(Agent::Custom),
            other => Err(format!("unknown agent: {other}")),
        }
    }
}

impl std::fmt::Display for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementStatus {
    Allowed,
    Caution,
    Blocked,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementSidedness {
    #[default]
    Bilateral,
    Unilateral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationSide {
    Left,
    Right,
    Bilateral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStateKind {
    Idle,
    Recommendation,
    Active,
    Cooldown,
}

impl AppStateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AppStateKind::Idle => "idle",
            AppStateKind::Recommendation => "recommendation",
            AppStateKind::Active => "active",
            AppStateKind::Cooldown => "cooldown",
        }
    }
}

impl std::str::FromStr for AppStateKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "idle" => Ok(AppStateKind::Idle),
            "recommendation" | "opportunity" => Ok(AppStateKind::Recommendation),
            "active" => Ok(AppStateKind::Active),
            "cooldown" => Ok(AppStateKind::Cooldown),
            other => Err(format!("unknown app state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movement {
    pub id: String,
    pub name: String,
    pub primary_muscle: String,
    pub muscles: Vec<String>,
    pub equipment: Vec<String>,
    pub base_reps: u32,
    pub estimated_seconds: u32,
    pub status: MovementStatus,
    pub mobility: bool,
    #[serde(default)]
    pub sidedness: MovementSidedness,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub agent: Agent,
    pub event: String,
    pub expected_duration_sec: u32,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommenderTokenUsage {
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommenderTokenProvider {
    Codex,
    OpenAi,
}

impl RecommenderTokenProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenAi => "openai",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecommenderTokenUsageSummary {
    pub today: TokenUsageTotals,
    pub week: TokenUsageTotals,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecommenderTokenUsageByProvider {
    pub codex: RecommenderTokenUsageSummary,
    pub openai: RecommenderTokenUsageSummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForgeActivityTotals {
    pub forges: u64,
    pub reps: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForgeActivitySummary {
    pub today: ForgeActivityTotals,
    pub week: ForgeActivityTotals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NutritionTotals {
    pub calories: f64,
    pub protein_g: f64,
    pub carbohydrates_g: f64,
    pub fat_g: f64,
    pub fiber_g: f64,
    pub sugar_g: f64,
    pub sodium_mg: f64,
    pub potassium_mg: f64,
}

impl NutritionTotals {
    pub fn add_assign(&mut self, other: &Self) {
        self.calories += other.calories;
        self.protein_g += other.protein_g;
        self.carbohydrates_g += other.carbohydrates_g;
        self.fat_g += other.fat_g;
        self.fiber_g += other.fiber_g;
        self.sugar_g += other.sugar_g;
        self.sodium_mg += other.sodium_mg;
        self.potassium_mg += other.potassium_mg;
    }

    pub fn scale_assign(&mut self, factor: f64) {
        self.calories *= factor;
        self.protein_g *= factor;
        self.carbohydrates_g *= factor;
        self.fat_g *= factor;
        self.fiber_g *= factor;
        self.sugar_g *= factor;
        self.sodium_mg *= factor;
        self.potassium_mg *= factor;
    }

    pub fn values(&self) -> [f64; 8] {
        [
            self.calories,
            self.protein_g,
            self.carbohydrates_g,
            self.fat_g,
            self.fiber_g,
            self.sugar_g,
            self.sodium_mg,
            self.potassium_mg,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuelItem {
    pub name: String,
    pub quantity: Option<f64>,
    pub unit: Option<String>,
    #[serde(flatten)]
    pub nutrition: NutritionTotals,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuelParseResult {
    pub items: Vec<FuelItem>,
}

impl FuelParseResult {
    pub fn totals(&self) -> NutritionTotals {
        let mut totals = NutritionTotals::default();
        for item in &self.items {
            totals.add_assign(&item.nutrition);
        }
        totals
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedFuelEvent {
    pub consumed_at: DateTime<Utc>,
    pub source_text: String,
    pub parsed: FuelParseResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuelEntry {
    pub id: i64,
    pub raw_text: String,
    pub parsed: FuelParseResult,
    pub totals: NutritionTotals,
    pub provider: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WaterTotal {
    pub milliliters: f64,
    pub fluid_ounces: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncomingEvent {
    pub agent: Agent,
    pub event: String,
    pub expected_duration_sec: Option<u32>,
    #[serde(alias = "duration")]
    pub duration_sec: Option<u32>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexHookEvent {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub cwd: String,
    pub hook_event_name: String,
    pub source: Option<String>,
    pub reason: Option<String>,
}

impl CodexHookEvent {
    pub fn validate(&self) -> Result<()> {
        validate_text("session_id", &self.session_id, 256)?;
        validate_optional_text("turn_id", self.turn_id.as_deref(), 256)?;
        validate_text("cwd", &self.cwd, 4096)?;
        validate_optional_text("source", self.source.as_deref(), 256)?;
        validate_optional_text("reason", self.reason.as_deref(), 1024)?;
        if !matches!(
            self.hook_event_name.as_str(),
            "SessionStart" | "UserPromptSubmit" | "Stop" | "SessionEnd"
        ) {
            bail!("unsupported Codex lifecycle event");
        }
        Ok(())
    }

    pub fn project(&self) -> Option<String> {
        std::path::Path::new(&self.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    }
}

impl IncomingEvent {
    pub fn validate(&self) -> Result<()> {
        validate_text("event", &self.event, 64)?;
        validate_optional_text("project", self.project.as_deref(), 256)?;
        for duration in [self.expected_duration_sec, self.duration_sec]
            .into_iter()
            .flatten()
        {
            if !(1..=86_400).contains(&duration) {
                bail!("duration must be between 1 and 86400 seconds");
            }
        }
        Ok(())
    }

    pub fn into_event_with_default(self, default_duration_sec: u32) -> AgentEvent {
        AgentEvent {
            agent: self.agent,
            event: self.event,
            expected_duration_sec: self
                .expected_duration_sec
                .or(self.duration_sec)
                .unwrap_or(default_duration_sec),
            project: self.project,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod input_validation_tests {
    use super::*;

    fn incoming() -> IncomingEvent {
        IncomingEvent {
            agent: Agent::Custom,
            event: "busy".into(),
            expected_duration_sec: Some(120),
            duration_sec: None,
            project: Some("svarog".into()),
        }
    }

    #[test]
    fn incoming_event_bounds_persisted_text_and_duration() {
        assert!(incoming().validate().is_ok());
        let mut invalid = incoming();
        invalid.event = "x".repeat(65);
        assert!(invalid.validate().is_err());
        invalid = incoming();
        invalid.project = Some("bad\nproject".into());
        assert!(invalid.validate().is_err());
        invalid = incoming();
        invalid.expected_duration_sec = Some(86_401);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn codex_hook_accepts_only_bounded_known_lifecycle_events() {
        let mut hook = CodexHookEvent {
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            cwd: "/work/svarog".into(),
            hook_event_name: "UserPromptSubmit".into(),
            source: None,
            reason: None,
        };
        assert!(hook.validate().is_ok());
        hook.hook_event_name = "Unknown".into();
        assert!(hook.validate().is_err());
        hook.hook_event_name = "Stop".into();
        hook.cwd = "x".repeat(4097);
        assert!(hook.validate().is_err());
    }
}

fn validate_optional_text(name: &str, value: Option<&str>, max_bytes: usize) -> Result<()> {
    if let Some(value) = value {
        validate_text(name, value, max_bytes)?;
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.len() > max_bytes {
        bail!("{name} must contain between 1 and {max_bytes} bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("{name} must not contain control characters");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: Option<i64>,
    pub movement_id: String,
    pub movement_name: String,
    pub primary_muscle: String,
    pub muscles: Vec<String>,
    pub reps: u32,
    pub weight_kg: Option<f32>,
    pub estimated_seconds: u32,
    pub agent: Agent,
    pub project: Option<String>,
    pub side: Option<RecommendationSide>,
    pub created_at: DateTime<Utc>,
}

impl Recommendation {
    pub fn display_name(&self) -> String {
        match self.side {
            Some(RecommendationSide::Left) => format!("{} (left side)", self.movement_name),
            Some(RecommendationSide::Right) => format!("{} (right side)", self.movement_name),
            Some(RecommendationSide::Bilateral) | None => self.movement_name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub kind: AppStateKind,
    pub current_recommendation_id: Option<i64>,
    pub cooldown_muscle: Option<String>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub suppress_until_event_count: Option<u32>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetStatus {
    Done,
    Skipped,
    Pain,
    Started,
}

impl SetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SetStatus::Done => "done",
            SetStatus::Skipped => "skipped",
            SetStatus::Pain => "pain",
            SetStatus::Started => "started",
        }
    }
}
