use anyhow::{Context, Result};
use minijinja::{context, Environment, UndefinedBehavior};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const EXERCISE_PROFILE_NAME: &str = "exercise_profile.j2";
const RECOMMENDATION_QUEUE_NAME: &str = "recommendation_queue.j2";
const DEFAULT_EXERCISE_PROFILE: &str = include_str!("../prompts/exercise_profile.j2");
const DEFAULT_RECOMMENDATION_QUEUE: &str = include_str!("../prompts/recommendation_queue.j2");

pub struct PromptRenderer<'a> {
    config_dir: &'a Path,
}

impl<'a> PromptRenderer<'a> {
    pub fn new(config_dir: &'a Path) -> Self {
        Self { config_dir }
    }

    pub fn exercise_profile<T: Serialize>(&self, config: &T) -> Result<String> {
        self.render(
            EXERCISE_PROFILE_NAME,
            DEFAULT_EXERCISE_PROFILE,
            context!(config => config),
        )
    }

    pub fn recommendation_queue<T: Serialize>(
        &self,
        context_value: &T,
        needed: u32,
    ) -> Result<String> {
        self.render(
            RECOMMENDATION_QUEUE_NAME,
            DEFAULT_RECOMMENDATION_QUEUE,
            context!(context => context_value, needed => needed),
        )
    }

    fn render(
        &self,
        name: &str,
        embedded_default: &str,
        values: minijinja::Value,
    ) -> Result<String> {
        let override_path = self.override_path(name);
        let source = if override_path.exists() {
            fs::read_to_string(&override_path)
                .with_context(|| format!("reading prompt override {}", override_path.display()))?
        } else {
            embedded_default.to_string()
        };
        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment
            .add_template_owned(name.to_string(), source)
            .with_context(|| format!("parsing prompt template {name}"))?;
        environment
            .get_template(name)
            .with_context(|| format!("loading prompt template {name}"))?
            .render(values)
            .with_context(|| format!("rendering prompt template {name}"))
    }

    fn override_path(&self, name: &str) -> PathBuf {
        self.config_dir.join("prompts").join(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn embedded_templates_render_structured_values() {
        let root = tempdir().unwrap();
        let renderer = PromptRenderer::new(root.path());

        let profile = renderer
            .exercise_profile(&json!({"profile": {"age": 33}}))
            .unwrap();
        let queue = renderer
            .recommendation_queue(&json!({"today_stats": {"reps": 8}}), 5)
            .unwrap();

        assert!(profile.starts_with(DEFAULT_EXERCISE_PROFILE.lines().next().unwrap()));
        assert!(profile.contains("\"age\":33"));
        assert!(!queue.contains("{{ needed }}"));
        assert!(queue.contains("\"reps\":8"));
    }

    #[test]
    fn override_edits_apply_to_the_next_render() {
        let root = tempdir().unwrap();
        let prompts = root.path().join("prompts");
        fs::create_dir_all(&prompts).unwrap();
        let path = prompts.join(RECOMMENDATION_QUEUE_NAME);
        fs::write(&path, "first {{ needed }} {{ context.name }}").unwrap();
        let renderer = PromptRenderer::new(root.path());

        assert_eq!(
            renderer
                .recommendation_queue(&json!({"name": "forge"}), 3)
                .unwrap(),
            "first 3 forge"
        );
        fs::write(&path, "second {{ needed }} {{ context.name }}").unwrap();
        assert_eq!(
            renderer
                .recommendation_queue(&json!({"name": "forge"}), 3)
                .unwrap(),
            "second 3 forge"
        );
    }

    #[test]
    fn invalid_override_variables_fail_strictly() {
        let root = tempdir().unwrap();
        let prompts = root.path().join("prompts");
        fs::create_dir_all(&prompts).unwrap();
        fs::write(prompts.join(EXERCISE_PROFILE_NAME), "{{ missing_value }}").unwrap();

        let error = PromptRenderer::new(root.path())
            .exercise_profile(&json!({}))
            .unwrap_err();

        assert!(format!("{error:#}").contains("undefined value"));
    }
}
