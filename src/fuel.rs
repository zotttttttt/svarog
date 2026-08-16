use crate::config::{Config, Paths, RecommenderBackend, UnitSystem};
use crate::models::{FuelParseResult, NutritionTotals};
use crate::prompt_templates::PromptRenderer;
use crate::recommender;
use crate::storage::Store;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;

pub const FUEL_MODEL: &str = "gpt-5.6-luna";
const MAX_FUEL_ITEMS: usize = 20;

#[derive(Debug, Clone)]
pub struct FuelParseOutcome {
    pub parsed: FuelParseResult,
    pub provider: &'static str,
    pub model: &'static str,
}

#[derive(Serialize)]
struct FuelPromptContext<'a> {
    input: &'a str,
    unit_system: UnitSystem,
    recent_entries: Vec<RecentFuelContext>,
}

#[derive(Serialize)]
struct RecentFuelContext {
    text: String,
    totals: NutritionTotals,
}

pub fn parse_fuel(
    store: &Store,
    config: &Config,
    paths: &Paths,
    input: &str,
) -> Result<FuelParseOutcome> {
    let input = input.trim();
    if input.is_empty() {
        bail!("describe a meal or drink first");
    }
    if input.chars().count() > 500 {
        bail!("meal or drink description is limited to 500 characters");
    }
    let recent_entries = store
        .recent_fuel_entries(10)?
        .into_iter()
        .map(|entry| RecentFuelContext {
            text: entry.raw_text,
            totals: entry.totals,
        })
        .collect();
    let context = FuelPromptContext {
        input,
        unit_system: config.profile.unit_system,
        recent_entries,
    };
    let prompt = PromptRenderer::new(&paths.config_dir).fuel_entry(&context)?;
    let schema = fuel_schema();
    let (parsed, provider) = match config.recommender.backend {
        RecommenderBackend::Codex => (
            recommender::call_codex_json_for_model(store, config, &prompt, &schema, FUEL_MODEL)
                .context("parsing meal or drink with Codex")?,
            "codex",
        ),
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
            let body = openai_fuel_request_body(config, &prompt, schema);
            (
                recommender::call_openai_json(store, config, paths, body)
                    .context("parsing meal or drink with OpenAI")?,
                "openai",
            )
        }
        RecommenderBackend::Local => {
            bail!(
                "meal and drink parsing needs Codex or an OpenAI backend; water is still available"
            )
        }
    };
    validate_parsed(&parsed)?;
    Ok(FuelParseOutcome {
        parsed,
        provider,
        model: FUEL_MODEL,
    })
}

fn openai_fuel_request_body(
    config: &Config,
    prompt: &str,
    schema: serde_json::Value,
) -> serde_json::Value {
    json!({
        "model": FUEL_MODEL,
        "reasoning": { "effort": config.recommender.openai.reasoning_effort },
        "input": [{ "role": "user", "content": prompt }],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "svarog_fuel_entry",
                "strict": true,
                "schema": schema
            }
        }
    })
}

fn validate_parsed(parsed: &FuelParseResult) -> Result<()> {
    if parsed.items.is_empty() {
        bail!("nutrition parser returned no food or drink items");
    }
    if parsed.items.len() > MAX_FUEL_ITEMS {
        bail!("nutrition parser returned too many items");
    }
    for item in &parsed.items {
        if item.name.trim().is_empty() || item.name.chars().count() > 120 {
            bail!("nutrition parser returned an invalid item name");
        }
        if item
            .quantity
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            bail!("nutrition parser returned an invalid quantity");
        }
        if item
            .unit
            .as_ref()
            .is_some_and(|unit| unit.chars().count() > 40)
        {
            bail!("nutrition parser returned an invalid unit");
        }
        if item
            .nutrition
            .values()
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            bail!("nutrition parser returned an invalid nutrition value");
        }
        if item.assumptions.len() > 8
            || item
                .assumptions
                .iter()
                .any(|assumption| assumption.chars().count() > 200)
        {
            bail!("nutrition parser returned invalid assumptions");
        }
    }
    Ok(())
}

fn fuel_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["items"],
        "properties": {
            "items": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_FUEL_ITEMS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "name", "quantity", "unit", "calories", "protein_g",
                        "carbohydrates_g", "fat_g", "fiber_g", "sugar_g",
                        "sodium_mg", "potassium_mg", "assumptions"
                    ],
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 120 },
                        "quantity": { "type": ["number", "null"], "minimum": 0 },
                        "unit": { "type": ["string", "null"], "maxLength": 40 },
                        "calories": { "type": "number", "minimum": 0 },
                        "protein_g": { "type": "number", "minimum": 0 },
                        "carbohydrates_g": { "type": "number", "minimum": 0 },
                        "fat_g": { "type": "number", "minimum": 0 },
                        "fiber_g": { "type": "number", "minimum": 0 },
                        "sugar_g": { "type": "number", "minimum": 0 },
                        "sodium_mg": { "type": "number", "minimum": 0 },
                        "potassium_mg": { "type": "number", "minimum": 0 },
                        "assumptions": {
                            "type": "array",
                            "maxItems": 8,
                            "items": { "type": "string", "maxLength": 200 }
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_strict_and_bounded() {
        let schema = fuel_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["items"]["maxItems"], 20);
        assert_eq!(
            schema["properties"]["items"]["items"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn openai_fuel_payload_always_uses_luna_and_strict_schema() {
        let mut config = Config::default();
        config.recommender.openai.model = "some-other-model".into();
        let body = openai_fuel_request_body(&config, "meal", fuel_schema());

        assert_eq!(body["model"], FUEL_MODEL);
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["text"]["format"]["type"], "json_schema");
    }
}
