use crate::config::{Config, Paths, RecommenderBackend, UnitSystem};
use crate::models::FuelParseResult;
use crate::prompt_templates::PromptRenderer;
use crate::recommender;
use crate::storage::Store;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
}

pub fn parse_fuel(
    store: &Store,
    config: &Config,
    paths: &Paths,
    input: &str,
    cancel: Arc<AtomicBool>,
) -> Result<FuelParseOutcome> {
    let input = input.trim();
    if input.is_empty() {
        bail!("describe a meal or drink first");
    }
    if input.chars().count() > 500 {
        bail!("meal or drink description is limited to 500 characters");
    }
    let context = FuelPromptContext {
        input,
        unit_system: config.profile.unit_system,
    };
    let prompt = PromptRenderer::new(&paths.config_dir).fuel_entry(&context)?;
    let schema = fuel_schema();
    let (parsed, provider) = match config.recommender.backend {
        RecommenderBackend::Codex => (
            recommender::call_codex_json_for_model(
                store,
                config,
                &prompt,
                &schema,
                FUEL_MODEL,
                cancel.as_ref(),
            )
            .context("parsing meal or drink with Codex")?,
            "codex",
        ),
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
            let body = openai_fuel_request_body(config, &prompt, schema);
            (
                recommender::call_openai_json_cancellable(store, config, paths, body, cancel)
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

pub(crate) fn validate_parsed(parsed: &FuelParseResult) -> Result<()> {
    if parsed.items.is_empty() {
        bail!("nutrition parser returned no food or drink items");
    }
    if parsed.items.len() > MAX_FUEL_ITEMS {
        bail!("nutrition parser returned too many items");
    }
    let mut names = HashSet::new();
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
            .is_some_and(|unit| unit.trim().is_empty() || unit.chars().count() > 40)
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
        if item.nutrition.calories > 20_000.0
            || [
                item.nutrition.protein_g,
                item.nutrition.carbohydrates_g,
                item.nutrition.fat_g,
                item.nutrition.fiber_g,
                item.nutrition.sugar_g,
            ]
            .iter()
            .any(|value| *value > 5_000.0)
            || item.nutrition.sodium_mg > 100_000.0
            || item.nutrition.potassium_mg > 100_000.0
            || item.nutrition.sugar_g > item.nutrition.carbohydrates_g
        {
            bail!("nutrition parser returned implausible nutrition values");
        }
        let normalized_name = item
            .name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !names.insert(normalized_name) {
            bail!("nutrition parser returned a duplicate item");
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
    let totals = parsed.totals();
    if totals.values().iter().any(|value| !value.is_finite())
        || totals.calories > 50_000.0
        || [
            totals.protein_g,
            totals.carbohydrates_g,
            totals.fat_g,
            totals.fiber_g,
            totals.sugar_g,
        ]
        .iter()
        .any(|value| *value > 10_000.0)
        || totals.sodium_mg > 500_000.0
        || totals.potassium_mg > 500_000.0
    {
        bail!("nutrition parser returned invalid aggregate totals");
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
                        "calories": { "type": "number", "minimum": 0, "maximum": 20000 },
                        "protein_g": { "type": "number", "minimum": 0, "maximum": 5000 },
                        "carbohydrates_g": { "type": "number", "minimum": 0, "maximum": 5000 },
                        "fat_g": { "type": "number", "minimum": 0, "maximum": 5000 },
                        "fiber_g": { "type": "number", "minimum": 0, "maximum": 5000 },
                        "sugar_g": { "type": "number", "minimum": 0, "maximum": 5000 },
                        "sodium_mg": { "type": "number", "minimum": 0, "maximum": 100000 },
                        "potassium_mg": { "type": "number", "minimum": 0, "maximum": 100000 },
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
    use crate::models::{FuelItem, NutritionTotals};

    fn parsed_item(name: &str) -> FuelParseResult {
        FuelParseResult {
            items: vec![FuelItem {
                name: name.into(),
                quantity: Some(1.0),
                unit: Some("serving".into()),
                nutrition: NutritionTotals {
                    calories: 400.0,
                    protein_g: 20.0,
                    carbohydrates_g: 50.0,
                    fat_g: 12.0,
                    fiber_g: 5.0,
                    sugar_g: 10.0,
                    sodium_mg: 500.0,
                    potassium_mg: 600.0,
                },
                assumptions: Vec::new(),
            }],
        }
    }

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

    #[test]
    fn prompt_context_contains_no_historical_entries() {
        let value = serde_json::to_value(FuelPromptContext {
            input: "oatmeal",
            unit_system: UnitSystem::Metric,
        })
        .unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value["input"], "oatmeal");
        assert!(value.get("recent_entries").is_none());
    }

    #[test]
    fn validation_rejects_duplicates_blank_units_and_cross_field_errors() {
        let mut duplicate = parsed_item("Greek yogurt");
        let mut second = duplicate.items[0].clone();
        second.name = "  greek   YOGURT ".into();
        duplicate.items.push(second);
        assert!(validate_parsed(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        let mut blank_unit = parsed_item("tea");
        blank_unit.items[0].unit = Some("  ".into());
        assert!(validate_parsed(&blank_unit).is_err());

        let mut sugar = parsed_item("dessert");
        sugar.items[0].nutrition.sugar_g = 60.0;
        assert!(validate_parsed(&sugar).is_err());
    }

    #[test]
    fn validation_rejects_item_and_aggregate_limits() {
        let mut item = parsed_item("large meal");
        item.items[0].nutrition.calories = 20_001.0;
        assert!(validate_parsed(&item).is_err());

        let mut aggregate = parsed_item("part 0");
        aggregate.items[0].nutrition.calories = 19_000.0;
        for index in 1..3 {
            let mut next = aggregate.items[0].clone();
            next.name = format!("part {index}");
            aggregate.items.push(next);
        }
        assert!(validate_parsed(&aggregate)
            .unwrap_err()
            .to_string()
            .contains("aggregate"));
    }
}
