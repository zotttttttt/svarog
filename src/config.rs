use crate::archetypes::ArchetypeId;
use anyhow::{bail, Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub profile: Profile,
    #[serde(default)]
    pub forge: Forge,
    pub agents: Agents,
    pub preferences: Preferences,
    #[serde(default)]
    pub recommender: Recommender,
    #[serde(default)]
    pub onboarding: Onboarding,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Onboarding {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub completed_steps: Vec<String>,
}

pub const CURRENT_ONBOARDING_VERSION: u32 = 3;
pub const STEP_HEIGHT: &str = "profile.height_cm";
pub const STEP_WEIGHT: &str = "profile.weight_kg";
pub const STEP_AGE: &str = "profile.age";
pub const STEP_GOALS: &str = "profile.goals";
pub const STEP_EQUIPMENT: &str = "profile.equipment";
pub const STEP_WORK_SETUP: &str = "profile.work_setup";
pub const STEP_ARM_AVAILABILITY: &str = "profile.arm_availability";
pub const STEP_CAUTIOUS_BODY_PARTS: &str = "profile.cautious_body_parts";
pub const STEP_INJURIES: &str = "profile.injuries";
pub const STEP_ARCHETYPE: &str = "forge.archetype";
pub const STEP_DESKTOP_NOTIFICATIONS: &str = "preferences.desktop_notifications";
pub const STEP_CODEX_COMMAND: &str = "agents.codex_command";
pub const STEP_EXERCISE_PREFERENCES: &str = "profile.exercise_preferences";
const LEGACY_STEP_RECOMMENDER_BACKEND: &str = "recommender.backend";
const LEGACY_STEP_FORGE_INTENSITY: &str = "preferences.forge_intensity";
const LEGACY_STEP_FORGE_FREQUENCY: &str = "preferences.forge_frequency";

// Keep this frozen for configs written before onboarding metadata existed.
pub const ORIGINAL_ONBOARDING_STEPS: [&str; 14] = [
    STEP_HEIGHT,
    STEP_WEIGHT,
    STEP_AGE,
    STEP_GOALS,
    STEP_EQUIPMENT,
    STEP_WORK_SETUP,
    STEP_ARM_AVAILABILITY,
    STEP_CAUTIOUS_BODY_PARTS,
    STEP_INJURIES,
    LEGACY_STEP_FORGE_INTENSITY,
    LEGACY_STEP_FORGE_FREQUENCY,
    STEP_CODEX_COMMAND,
    STEP_EXERCISE_PREFERENCES,
    LEGACY_STEP_RECOMMENDER_BACKEND,
];

// Add every new question here with a stable ID and increment the version above.
pub const CURRENT_ONBOARDING_STEPS: [&str; 12] = [
    STEP_HEIGHT,
    STEP_WEIGHT,
    STEP_AGE,
    STEP_GOALS,
    STEP_EQUIPMENT,
    STEP_WORK_SETUP,
    STEP_ARM_AVAILABILITY,
    STEP_CAUTIOUS_BODY_PARTS,
    STEP_INJURIES,
    STEP_DESKTOP_NOTIFICATIONS,
    STEP_EXERCISE_PREFERENCES,
    STEP_ARCHETYPE,
];

impl Onboarding {
    pub fn is_completed(&self, step: &str) -> bool {
        self.completed_steps
            .iter()
            .any(|completed| completed == step)
    }

    pub fn mark_completed(&mut self, step: &str) {
        if !self.is_completed(step) {
            self.completed_steps.push(step.to_string());
        }
        self.reconcile_version();
    }

    pub fn reconcile_version(&mut self) {
        if CURRENT_ONBOARDING_STEPS
            .iter()
            .all(|required| self.is_completed(required))
        {
            self.version = CURRENT_ONBOARDING_VERSION;
        }
    }

    pub fn pending_steps(&self) -> Vec<&'static str> {
        CURRENT_ONBOARDING_STEPS
            .iter()
            .copied()
            .filter(|step| !self.is_completed(step))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.pending_steps().is_empty() && self.version >= CURRENT_ONBOARDING_VERSION
    }

    fn mark_legacy_original_steps_completed(&mut self) {
        for step in ORIGINAL_ONBOARDING_STEPS {
            self.mark_completed(step);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub unit_system: UnitSystem,
    pub height_cm: Option<u32>,
    pub weight_kg: Option<f32>,
    pub age: Option<u32>,
    pub goals: Vec<String>,
    #[serde(default = "default_equipment_text")]
    pub equipment_text: String,
    #[serde(default = "default_exercise_preferences")]
    pub exercise_preferences: String,
    #[serde(default = "default_work_setup")]
    pub work_setup: String,
    pub one_hand_available: bool,
    #[serde(default = "default_true")]
    pub two_hand_available: bool,
    pub cautious_body_parts: Vec<String>,
    #[serde(default)]
    pub injuries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forge {
    #[serde(default)]
    pub archetype: ArchetypeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_archetype: Option<String>,
}

impl Default for Forge {
    fn default() -> Self {
        Self {
            archetype: ArchetypeId::Athlete,
            custom_archetype: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitSystem {
    #[default]
    Metric,
    Imperial,
}

impl fmt::Display for UnitSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Metric => "metric",
            Self::Imperial => "imperial",
        })
    }
}

impl FromStr for UnitSystem {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "metric" | "m" => Ok(Self::Metric),
            "imperial" | "i" => Ok(Self::Imperial),
            _ => Err("use one of: metric, imperial".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agents {
    pub codex_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub default_expected_duration_sec: u32,
    pub max_daily_sets: u32,
    #[serde(default = "default_true")]
    pub desktop_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommender {
    pub backend: RecommenderBackend,
    pub timeout_ms: u64,
    pub local_fallback: bool,
    pub show_llm_failures: bool,
    pub codex: CodexRecommender,
    pub openai: OpenAiRecommender,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommenderBackend {
    Codex,
    #[serde(rename = "openai_env", alias = "openai")]
    OpenaiEnv,
    OpenaiKeyring,
    #[serde(alias = "off")]
    Local,
}

impl RecommenderBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::OpenaiEnv => "OpenAI (environment)",
            Self::OpenaiKeyring => "OpenAI (saved key)",
            Self::Local => "Local",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Local => Self::OpenaiEnv,
            Self::OpenaiEnv => Self::OpenaiKeyring,
            Self::OpenaiKeyring => Self::Codex,
            Self::Codex => Self::Local,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Local => Self::Codex,
            Self::Codex => Self::OpenaiKeyring,
            Self::OpenaiKeyring => Self::OpenaiEnv,
            Self::OpenaiEnv => Self::Local,
        }
    }
}

impl FromStr for RecommenderBackend {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "codex" => Ok(Self::Codex),
            "openai" | "openai env" | "openai_env" | "openai (environment)" => Ok(Self::OpenaiEnv),
            "openai keyring" | "openai_keyring" | "openai saved" | "openai (saved key)" => {
                Ok(Self::OpenaiKeyring)
            }
            "local" => Ok(Self::Local),
            _ => Err("use one of: codex, openai_env, openai_keyring, local".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRecommender {
    pub command: String,
    pub args: Vec<String>,
    #[serde(default = "default_codex_recommender_model")]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiRecommender {
    pub api_key_env: String,
    pub model: String,
    pub reasoning_effort: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: Profile {
                unit_system: UnitSystem::Metric,
                height_cm: None,
                weight_kg: None,
                age: None,
                goals: vec!["consistent movement".to_string()],
                equipment_text: default_equipment_text(),
                exercise_preferences: default_exercise_preferences(),
                work_setup: default_work_setup(),
                one_hand_available: true,
                two_hand_available: true,
                cautious_body_parts: Vec::new(),
                injuries: Vec::new(),
            },
            forge: Forge::default(),
            agents: Agents {
                codex_command: "codex".to_string(),
            },
            preferences: Preferences {
                default_expected_duration_sec: 60,
                max_daily_sets: 100,
                desktop_notifications: true,
            },
            recommender: Recommender::default(),
            onboarding: Onboarding::default(),
        }
    }
}

impl Default for Recommender {
    fn default() -> Self {
        Self {
            backend: RecommenderBackend::Local,
            timeout_ms: 60_000,
            local_fallback: true,
            show_llm_failures: true,
            codex: CodexRecommender {
                command: "codex".to_string(),
                args: vec![
                    "exec".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--sandbox".to_string(),
                    "read-only".to_string(),
                    "--disable".to_string(),
                    "apps".to_string(),
                    "--disable".to_string(),
                    "plugins".to_string(),
                    "--disable".to_string(),
                    "skill_search".to_string(),
                    "--disable".to_string(),
                    "tool_suggest".to_string(),
                    "--disable".to_string(),
                    "multi_agent".to_string(),
                    "--disable".to_string(),
                    "browser_use".to_string(),
                    "--disable".to_string(),
                    "computer_use".to_string(),
                    "--disable".to_string(),
                    "image_generation".to_string(),
                    "--disable".to_string(),
                    "hooks".to_string(),
                ],
                model: default_codex_recommender_model(),
            },
            openai: OpenAiRecommender {
                api_key_env: "OPENAI_API_KEY".to_string(),
                model: default_openai_recommender_model(),
                reasoning_effort: "low".to_string(),
            },
        }
    }
}

fn default_work_setup() -> String {
    "sitting".to_string()
}

fn default_equipment_text() -> String {
    "bodyweight only".to_string()
}

fn default_exercise_preferences() -> String {
    "automatic".to_string()
}

fn default_true() -> bool {
    true
}

fn default_codex_recommender_model() -> String {
    "gpt-5.6-luna".to_string()
}

fn default_openai_recommender_model() -> String {
    "gpt-5.6-luna".to_string()
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub database_file: PathBuf,
    pub credential_scope: CredentialScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialScope {
    Production,
    Development,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Production,
    Dev,
}

#[derive(Debug, Clone)]
pub struct RuntimeEnv {
    pub mode: RuntimeMode,
    pub paths: Paths,
    pub codex_home: PathBuf,
    pub daemon_addr: SocketAddr,
    pub dry_run: bool,
}

impl Paths {
    pub fn collector_token_file(&self) -> PathBuf {
        self.config_dir.join("collector.token")
    }

    pub fn load() -> Result<Self> {
        let dirs = BaseDirs::new().context("could not determine user directories")?;
        let config_dir = dirs.home_dir().join(".config").join("svarog");
        let data_dir = dirs.home_dir().join(".local").join("share").join("svarog");
        Ok(Self {
            config_file: config_dir.join("config.toml"),
            database_file: data_dir.join("svarog.sqlite3"),
            config_dir,
            data_dir,
            credential_scope: CredentialScope::Production,
        })
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            config_file: root.join("config.toml"),
            database_file: root.join("svarog.sqlite3"),
            config_dir: root.clone(),
            data_dir: root,
            credential_scope: CredentialScope::Development,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("creating {}", self.config_dir.display()))?;
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating {}", self.data_dir.display()))?;
        secure_path(&self.config_dir, 0o700)?;
        secure_path(&self.data_dir, 0o700)?;
        secure_file_if_exists(&self.config_file)?;
        secure_file_if_exists(&self.database_file)?;
        Ok(())
    }
}

fn secure_path(path: &std::path::Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("securing {}", path.display()))
}

fn secure_file_if_exists(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        secure_path(path, 0o600)?;
    }
    Ok(())
}

impl RuntimeEnv {
    pub fn load() -> Result<Self> {
        let dev = std::env::var("SVAROG_MODE").is_ok_and(|value| value == "dev");
        Self::load_with_options(dev, false)
    }

    pub fn load_with_options(dev: bool, dry_run: bool) -> Result<Self> {
        let mode = if dev {
            RuntimeMode::Dev
        } else {
            RuntimeMode::Production
        };
        let paths = resolve_svarog_paths(mode)?;
        let codex_home = resolve_codex_home(mode)?;
        let daemon_addr = resolve_daemon_addr(mode)?;
        Ok(Self {
            mode,
            paths,
            codex_home,
            daemon_addr,
            dry_run,
        })
    }

    pub fn load_demo() -> Result<Self> {
        let project_root = std::env::current_dir().context("determining current directory")?;
        Ok(Self::demo_for_project(project_root))
    }

    fn demo_for_project(project_root: PathBuf) -> Self {
        let root = project_root.join(".svarog-dev");
        Self {
            mode: RuntimeMode::Dev,
            paths: Paths::from_root(root.join("svarog")),
            codex_home: root.join("codex"),
            daemon_addr: "127.0.0.1:18787".parse().unwrap(),
            dry_run: false,
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            RuntimeMode::Production => "production",
            RuntimeMode::Dev => "dev sandbox",
        }
    }

    pub fn env_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "SVAROG_HOME",
                self.paths.config_dir.to_string_lossy().to_string(),
            ),
            ("CODEX_HOME", self.codex_home.to_string_lossy().to_string()),
            ("SVAROG_DAEMON_ADDR", self.daemon_addr.to_string()),
            (
                "SVAROG_MODE",
                match self.mode {
                    RuntimeMode::Production => "production",
                    RuntimeMode::Dev => "dev",
                }
                .to_string(),
            ),
        ]
    }
}

fn resolve_svarog_paths(mode: RuntimeMode) -> Result<Paths> {
    if let Ok(root) = std::env::var("SVAROG_HOME") {
        let mut paths = Paths::from_root(PathBuf::from(root));
        paths.credential_scope = match mode {
            RuntimeMode::Production => CredentialScope::Production,
            RuntimeMode::Dev => CredentialScope::Development,
        };
        return Ok(paths);
    }
    match mode {
        RuntimeMode::Production => Paths::load(),
        RuntimeMode::Dev => Ok(Paths::from_root(
            std::env::current_dir()
                .context("determining current directory")?
                .join(".svarog-dev")
                .join("svarog"),
        )),
    }
}

fn resolve_codex_home(mode: RuntimeMode) -> Result<PathBuf> {
    if let Ok(root) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(root));
    }
    match mode {
        RuntimeMode::Production => {
            let dirs = BaseDirs::new().context("could not determine user directories")?;
            Ok(dirs.home_dir().join(".codex"))
        }
        RuntimeMode::Dev => Ok(std::env::current_dir()
            .context("determining current directory")?
            .join(".svarog-dev")
            .join("codex")),
    }
}

fn resolve_daemon_addr(mode: RuntimeMode) -> Result<SocketAddr> {
    if let Ok(addr) = std::env::var("SVAROG_DAEMON_ADDR") {
        let parsed = addr
            .parse()
            .with_context(|| format!("parsing SVAROG_DAEMON_ADDR={addr}"))?;
        return validate_daemon_addr(parsed);
    }
    let addr = match mode {
        RuntimeMode::Production => "127.0.0.1:8787".parse().unwrap(),
        RuntimeMode::Dev => "127.0.0.1:18787".parse().unwrap(),
    };
    validate_daemon_addr(addr)
}

fn validate_daemon_addr(addr: SocketAddr) -> Result<SocketAddr> {
    if !addr.ip().is_loopback() {
        bail!("SVAROG_DAEMON_ADDR must use a loopback address because the event API is local-only");
    }
    Ok(addr)
}

pub fn load_or_default(paths: &Paths) -> Result<Config> {
    if !paths.config_file.exists() {
        return Ok(Config::default());
    }
    let contents = fs::read_to_string(&paths.config_file)
        .with_context(|| format!("reading {}", paths.config_file.display()))?;
    let legacy_without_onboarding = toml::from_str::<toml::Value>(&contents)
        .ok()
        .and_then(|root| root.get("onboarding").cloned())
        .is_none();
    let mut config: Config = toml::from_str(&contents)
        .with_context(|| format!("parsing {}", paths.config_file.display()))?;
    if legacy_without_onboarding {
        config.onboarding.mark_legacy_original_steps_completed();
    }
    config.onboarding.reconcile_version();
    normalize_loaded_config(&mut config);
    Ok(config)
}

pub fn save(paths: &Paths, config: &Config) -> Result<()> {
    paths.ensure()?;
    let contents = toml::to_string_pretty(config).context("serializing config")?;
    let mut temp = tempfile::NamedTempFile::new_in(&paths.config_dir)
        .with_context(|| format!("creating temporary file in {}", paths.config_dir.display()))?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| {
            format!(
                "securing temporary config in {}",
                paths.config_dir.display()
            )
        })?;
    temp.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", paths.config_file.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing {}", paths.config_file.display()))?;
    temp.persist(&paths.config_file)
        .with_context(|| format!("replacing {}", paths.config_file.display()))?;
    Ok(())
}

fn normalize_loaded_config(config: &mut Config) {
    if config.onboarding.version < 3 && config.preferences.max_daily_sets == 12 {
        config.preferences.max_daily_sets = 100;
    }
    if config.forge.archetype != ArchetypeId::Custom {
        config.forge.custom_archetype = None;
    } else if config
        .forge
        .custom_archetype
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        config.forge = Forge::default();
    }
    let old_codex_args = [
        "exec",
        "--ask-for-approval",
        "never",
        "--sandbox",
        "read-only",
    ];
    let previous_codex_args = ["exec", "--sandbox", "read-only"];
    let git_check_codex_args = ["exec", "--skip-git-repo-check", "--sandbox", "read-only"];
    if config
        .recommender
        .codex
        .args
        .iter()
        .map(String::as_str)
        .eq(old_codex_args)
        || config
            .recommender
            .codex
            .args
            .iter()
            .map(String::as_str)
            .eq(previous_codex_args)
        || config
            .recommender
            .codex
            .args
            .iter()
            .map(String::as_str)
            .eq(git_check_codex_args)
    {
        config.recommender.codex.args = Config::default().recommender.codex.args;
    }
    if config.recommender.timeout_ms == 8_000 {
        config.recommender.timeout_ms = 60_000;
    }
    if config.recommender.openai.model == "gpt-5.4-nano" {
        config.recommender.openai.model = default_openai_recommender_model();
    }
    if let Some(model) = take_codex_model_arg(&mut config.recommender.codex.args) {
        config.recommender.codex.model = model;
    }
}

fn take_codex_model_arg(args: &mut Vec<String>) -> Option<String> {
    let mut model = None;
    let mut retained = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "-m" || argument == "--model" {
            if let Some(value) = args.get(index + 1) {
                model = Some(value.clone());
                index += 2;
                continue;
            }
        } else if let Some(value) = argument.strip_prefix("--model=") {
            model = Some(value.to_string());
            index += 1;
            continue;
        }
        retained.push(argument.clone());
        index += 1;
    }
    *args = retained;
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    #[test]
    fn legacy_profile_defaults_to_metric_units() {
        let serialized = toml::to_string_pretty(&Config::default()).unwrap();
        let legacy = serialized
            .lines()
            .filter(|line| !line.starts_with("unit_system ="))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed: Config = toml::from_str(&legacy).unwrap();

        assert_eq!(parsed.profile.unit_system, UnitSystem::Metric);
    }

    #[test]
    fn unit_system_round_trips_as_lowercase_config_values() {
        let mut config = Config::default();
        config.profile.unit_system = UnitSystem::Imperial;

        let serialized = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&serialized).unwrap();

        assert!(serialized.contains("unit_system = \"imperial\""));
        assert_eq!(parsed.profile.unit_system, UnitSystem::Imperial);
        assert_eq!("m".parse::<UnitSystem>().unwrap(), UnitSystem::Metric);
        assert_eq!("i".parse::<UnitSystem>().unwrap(), UnitSystem::Imperial);
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn clear_runtime_env() {
        std::env::remove_var("SVAROG_HOME");
        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("SVAROG_DAEMON_ADDR");
        std::env::remove_var("SVAROG_MODE");
    }

    #[test]
    fn saves_and_loads_config() {
        let root = tempdir().unwrap().keep();
        let paths = Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            config_file: root.join("config").join("config.toml"),
            database_file: root.join("data").join("svarog.sqlite3"),
            credential_scope: CredentialScope::Development,
        };
        let mut config = Config::default();
        config.agents.codex_command = "codex --sandbox workspace-write".to_string();
        config.preferences.desktop_notifications = false;

        save(&paths, &config).unwrap();
        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(
            loaded.agents.codex_command,
            "codex --sandbox workspace-write"
        );
        assert!(!loaded.preferences.desktop_notifications);
        assert_eq!(
            fs::metadata(&paths.config_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.config_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&paths.data_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn default_setup_values_are_conservative() {
        let config = Config::default();

        assert_eq!(config.forge.archetype, ArchetypeId::Athlete);
        assert_eq!(config.preferences.max_daily_sets, 100);
        assert!(config.preferences.desktop_notifications);
        assert_eq!(config.profile.exercise_preferences, "automatic");
        assert_eq!(config.recommender.backend, RecommenderBackend::Local);
        for feature in [
            "apps",
            "plugins",
            "skill_search",
            "tool_suggest",
            "multi_agent",
            "browser_use",
            "computer_use",
            "image_generation",
            "hooks",
        ] {
            assert!(config
                .recommender
                .codex
                .args
                .windows(2)
                .any(|args| args == ["--disable", feature]));
        }
        assert_eq!(config.recommender.codex.model, "gpt-5.6-luna");
        assert_eq!(config.recommender.openai.model, "gpt-5.6-luna");
        assert_eq!(config.recommender.timeout_ms, 60_000);
        assert_eq!(config.onboarding.pending_steps(), CURRENT_ONBOARDING_STEPS);
    }

    #[test]
    fn load_migrates_only_the_old_default_openai_model() {
        let root = tempdir().unwrap();
        let old_paths = Paths::from_root(root.path().join("old"));
        let mut old = Config::default();
        old.recommender.openai.model = "gpt-5.4-nano".into();
        save(&old_paths, &old).unwrap();

        let custom_paths = Paths::from_root(root.path().join("custom"));
        let mut custom = Config::default();
        custom.recommender.openai.model = "custom-openai-model".into();
        save(&custom_paths, &custom).unwrap();

        assert_eq!(
            load_or_default(&old_paths)
                .unwrap()
                .recommender
                .openai
                .model,
            "gpt-5.6-luna"
        );
        assert_eq!(
            load_or_default(&custom_paths)
                .unwrap()
                .recommender
                .openai
                .model,
            "custom-openai-model"
        );
    }

    #[test]
    fn legacy_off_recommender_migrates_to_local() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let serialized = toml::to_string_pretty(&Config::default())
            .unwrap()
            .replace("backend = \"local\"", "backend = \"off\"");
        paths.ensure().unwrap();
        fs::write(&paths.config_file, serialized).unwrap();

        let config = load_or_default(&paths).unwrap();

        assert_eq!(config.recommender.backend, RecommenderBackend::Local);
        save(&paths, &config).unwrap();
        let migrated = fs::read_to_string(&paths.config_file).unwrap();
        assert!(migrated.contains("backend = \"local\""));
        assert!(!migrated.contains("backend = \"off\""));
    }

    #[test]
    fn legacy_openai_backend_migrates_to_explicit_environment_source() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let serialized = toml::to_string_pretty(&Config::default())
            .unwrap()
            .replace("backend = \"local\"", "backend = \"openai\"");
        paths.ensure().unwrap();
        fs::write(&paths.config_file, serialized).unwrap();

        let config = load_or_default(&paths).unwrap();

        assert_eq!(config.recommender.backend, RecommenderBackend::OpenaiEnv);
        save(&paths, &config).unwrap();
        let migrated = fs::read_to_string(&paths.config_file).unwrap();
        assert!(migrated.contains("backend = \"openai_env\""));
        assert!(!migrated.contains("backend = \"openai\""));
    }

    #[test]
    fn codex_command_is_configurable_but_not_a_current_onboarding_step() {
        assert!(!CURRENT_ONBOARDING_STEPS.contains(&STEP_CODEX_COMMAND));
        assert!(ORIGINAL_ONBOARDING_STEPS.contains(&STEP_CODEX_COMMAND));

        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.agents.codex_command = "custom-codex".into();
        save(&paths, &config).unwrap();

        assert_eq!(
            load_or_default(&paths).unwrap().agents.codex_command,
            "custom-codex"
        );
    }

    #[test]
    fn load_migrates_only_the_old_default_timeout() {
        let root = tempdir().unwrap();
        let old_paths = Paths::from_root(root.path().join("old"));
        let mut old = Config::default();
        old.recommender.timeout_ms = 8_000;
        save(&old_paths, &old).unwrap();

        let custom_paths = Paths::from_root(root.path().join("custom"));
        let mut custom = Config::default();
        custom.recommender.timeout_ms = 12_000;
        save(&custom_paths, &custom).unwrap();

        assert_eq!(
            load_or_default(&old_paths).unwrap().recommender.timeout_ms,
            60_000
        );
        assert_eq!(
            load_or_default(&custom_paths)
                .unwrap()
                .recommender
                .timeout_ms,
            12_000
        );
    }

    #[test]
    fn load_moves_a_legacy_model_argument_to_the_model_setting() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        paths.ensure().unwrap();
        let serialized = toml::to_string_pretty(&Config::default()).unwrap();
        let mut value: toml::Value = toml::from_str(&serialized).unwrap();
        let codex = value
            .get_mut("recommender")
            .and_then(|value| value.get_mut("codex"))
            .and_then(toml::Value::as_table_mut)
            .unwrap();
        codex.remove("model");
        codex.insert(
            "args".into(),
            toml::Value::Array(
                ["exec", "--model", "custom-codex-model"]
                    .into_iter()
                    .map(|value| toml::Value::String(value.into()))
                    .collect(),
            ),
        );
        fs::write(&paths.config_file, toml::to_string_pretty(&value).unwrap()).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(loaded.recommender.codex.model, "custom-codex-model");
        assert_eq!(loaded.recommender.codex.args, vec!["exec"]);
    }

    #[test]
    fn legacy_config_defaults_notifications_on_and_requests_only_the_new_question() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        paths.ensure().unwrap();
        let serialized = toml::to_string_pretty(&Config::default()).unwrap();
        let mut value: toml::Value = toml::from_str(&serialized).unwrap();
        value.as_table_mut().unwrap().remove("onboarding");
        value
            .get_mut("preferences")
            .and_then(toml::Value::as_table_mut)
            .unwrap()
            .remove("desktop_notifications");
        fs::write(&paths.config_file, toml::to_string_pretty(&value).unwrap()).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert!(loaded.preferences.desktop_notifications);
        assert_eq!(
            loaded.onboarding.pending_steps(),
            vec![STEP_DESKTOP_NOTIFICATIONS, STEP_ARCHETYPE]
        );
        assert!(!loaded.onboarding.is_complete());
    }

    #[test]
    fn onboarding_reports_only_the_missing_question() {
        let mut onboarding = Onboarding::default();
        for step in CURRENT_ONBOARDING_STEPS {
            if step != STEP_INJURIES {
                onboarding.mark_completed(step);
            }
        }

        assert_eq!(onboarding.pending_steps(), vec![STEP_INJURIES]);
        assert!(!onboarding.is_complete());

        onboarding.mark_completed(STEP_INJURIES);

        assert!(onboarding.is_complete());
        assert_eq!(onboarding.version, CURRENT_ONBOARDING_VERSION);
    }

    #[test]
    fn onboarding_ends_with_notifications_preferences_then_archetype() {
        assert_eq!(
            &CURRENT_ONBOARDING_STEPS[CURRENT_ONBOARDING_STEPS.len() - 3..],
            [
                STEP_DESKTOP_NOTIFICATIONS,
                STEP_EXERCISE_PREFERENCES,
                STEP_ARCHETYPE,
            ]
        );
    }

    #[test]
    fn version_two_default_ceiling_migrates_and_requests_archetype() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.preferences.max_daily_sets = 12;
        config.onboarding.version = 2;
        for step in CURRENT_ONBOARDING_STEPS {
            if step != STEP_ARCHETYPE {
                config.onboarding.mark_completed(step);
            }
        }
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(loaded.preferences.max_daily_sets, 100);
        assert_eq!(loaded.onboarding.pending_steps(), vec![STEP_ARCHETYPE]);
        assert_eq!(loaded.forge.archetype, ArchetypeId::Athlete);
    }

    #[test]
    fn empty_custom_archetype_falls_back_to_athlete() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.forge.archetype = ArchetypeId::Custom;
        config.forge.custom_archetype = Some("  ".into());
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();
        assert_eq!(loaded.forge, Forge::default());
    }

    #[test]
    fn load_normalizes_old_codex_exec_args() {
        let root = tempdir().unwrap().keep();
        let paths = Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            config_file: root.join("config").join("config.toml"),
            database_file: root.join("data").join("svarog.sqlite3"),
            credential_scope: CredentialScope::Development,
        };
        let mut config = Config::default();
        config.recommender.codex.args = vec![
            "exec".to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
        ];
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(
            loaded.recommender.codex.args,
            Config::default().recommender.codex.args
        );
    }

    #[test]
    fn load_normalizes_previous_default_codex_exec_args() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.recommender.codex.args = vec![
            "exec".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
        ];
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(
            loaded.recommender.codex.args,
            Config::default().recommender.codex.args
        );
    }

    #[test]
    fn load_normalizes_git_check_codex_exec_args_to_lean_defaults() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.recommender.codex.args = vec![
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--sandbox".into(),
            "read-only".into(),
        ];
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(
            loaded.recommender.codex.args,
            Config::default().recommender.codex.args
        );
    }

    #[test]
    fn load_preserves_custom_codex_exec_args() {
        let root = tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let mut config = Config::default();
        config.recommender.codex.args = vec!["exec".into(), "--ephemeral".into()];
        save(&paths, &config).unwrap();

        let loaded = load_or_default(&paths).unwrap();

        assert_eq!(loaded.recommender.codex.args, vec!["exec", "--ephemeral"]);
    }

    #[test]
    fn dev_runtime_uses_sandbox_paths_and_port() {
        let _guard = env_lock();
        clear_runtime_env();

        let env = RuntimeEnv::load_with_options(true, true).unwrap();

        assert_eq!(env.mode, RuntimeMode::Dev);
        assert!(env
            .paths
            .config_file
            .ends_with(".svarog-dev/svarog/config.toml"));
        assert!(env.codex_home.ends_with(".svarog-dev/codex"));
        assert_eq!(env.daemon_addr.to_string(), "127.0.0.1:18787");
        assert!(env.dry_run);
        clear_runtime_env();
    }

    #[test]
    fn runtime_env_vars_override_roots_and_addr() {
        let _guard = env_lock();
        clear_runtime_env();
        let root = tempdir().unwrap().keep();
        std::env::set_var("SVAROG_HOME", root.join("svarog"));
        std::env::set_var("CODEX_HOME", root.join("codex"));
        std::env::set_var("SVAROG_DAEMON_ADDR", "127.0.0.1:19999");

        let env = RuntimeEnv::load_with_options(false, false).unwrap();

        assert_eq!(
            env.paths.config_file,
            root.join("svarog").join("config.toml")
        );
        assert_eq!(env.codex_home, root.join("codex"));
        assert_eq!(env.daemon_addr.to_string(), "127.0.0.1:19999");
        clear_runtime_env();
    }

    #[test]
    fn runtime_rejects_non_loopback_daemon_addresses() {
        let _guard = env_lock();
        clear_runtime_env();
        std::env::set_var("SVAROG_DAEMON_ADDR", "0.0.0.0:8787");

        let error = RuntimeEnv::load_with_options(false, false).unwrap_err();

        assert!(error.to_string().contains("must use a loopback address"));
        clear_runtime_env();
    }

    #[test]
    fn runtime_env_pairs_propagate_mode_and_paths() {
        let _guard = env_lock();
        clear_runtime_env();
        let env = RuntimeEnv::load_with_options(true, false).unwrap();
        let pairs = env.env_pairs();

        assert!(pairs
            .iter()
            .any(|(key, value)| *key == "SVAROG_MODE" && value == "dev"));
        assert!(pairs.iter().any(|(key, _)| *key == "SVAROG_HOME"));
        assert!(pairs.iter().any(|(key, _)| *key == "CODEX_HOME"));
        assert!(pairs
            .iter()
            .any(|(key, value)| *key == "SVAROG_DAEMON_ADDR" && value == "127.0.0.1:18787"));
        clear_runtime_env();
    }

    #[test]
    fn demo_runtime_ignores_environment_overrides() {
        let _guard = env_lock();
        clear_runtime_env();
        let project = tempdir().unwrap();
        let production = tempdir().unwrap();
        std::env::set_var("SVAROG_HOME", production.path().join("svarog"));
        std::env::set_var("CODEX_HOME", production.path().join("codex"));
        std::env::set_var("SVAROG_DAEMON_ADDR", "127.0.0.1:8787");

        let env = RuntimeEnv::demo_for_project(project.path().to_path_buf());

        assert_eq!(
            env.paths.config_dir,
            project.path().join(".svarog-dev/svarog")
        );
        assert_eq!(env.codex_home, project.path().join(".svarog-dev/codex"));
        assert_eq!(env.daemon_addr.to_string(), "127.0.0.1:18787");
        clear_runtime_env();
    }
}
