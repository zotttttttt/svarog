use crate::config::{Config, Paths, RecommenderBackend, UnitSystem};
use crate::models::{FuelItem, FuelParseResult, NutritionTotals, TimedFuelEvent};
use crate::prompt_templates::PromptRenderer;
use crate::recommender;
use crate::storage::Store;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const FUEL_MODEL: &str = "gpt-5.6-luna";
const MAX_FUEL_ITEMS: usize = 20;
const MAX_FUEL_EVENTS: usize = 12;
const MAX_BATCH_ITEMS: usize = 40;
const MAX_EVENT_SOURCE_CHARS: usize = 500;
const YESTERDAY_INFERENCE_CUTOFF_HOUR: u32 = 4;
pub const MAX_FUEL_INPUT_CHARS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct FuelParseOutcome {
    pub events: Vec<TimedFuelEvent>,
    pub inferred_yesterday: bool,
    pub provider: &'static str,
    pub model: &'static str,
}

impl FuelParseOutcome {
    pub fn totals(&self) -> NutritionTotals {
        let mut totals = NutritionTotals::default();
        for event in &self.events {
            totals.add_assign(&event.parsed.totals());
        }
        totals
    }
}

#[derive(Debug, Deserialize)]
struct FuelTimelineParseResult {
    date: Option<String>,
    multiple_dates: bool,
    events: Vec<FuelTimelineEvent>,
}

#[derive(Debug, Deserialize)]
struct FuelTimelineEvent {
    time: Option<String>,
    inherit_previous_time: bool,
    source_text: String,
    items: Vec<FuelItem>,
}

#[derive(Serialize)]
struct FuelPromptContext<'a> {
    input: &'a str,
    unit_system: UnitSystem,
    local_now: String,
    timezone: String,
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
    if input.chars().count() > MAX_FUEL_INPUT_CHARS {
        bail!("meal or drink description is limited to {MAX_FUEL_INPUT_CHARS} characters");
    }
    let now = Local::now();
    let context = FuelPromptContext {
        input,
        unit_system: config.profile.unit_system,
        local_now: now.to_rfc3339(),
        timezone: now.format("%Z").to_string(),
    };
    let prompt = PromptRenderer::new(&paths.config_dir).fuel_entry(&context)?;
    let schema = fuel_schema();
    let (timeline, provider): (FuelTimelineParseResult, &'static str) =
        match config.recommender.backend {
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
    let (events, inferred_yesterday) = resolve_timeline_at(timeline, now)?;
    Ok(FuelParseOutcome {
        events,
        inferred_yesterday,
        provider,
        model: FUEL_MODEL,
    })
}

fn resolve_timeline_at(
    timeline: FuelTimelineParseResult,
    now: DateTime<Local>,
) -> Result<(Vec<TimedFuelEvent>, bool)> {
    if timeline.multiple_dates {
        bail!("describe fuel for one calendar date at a time");
    }
    if timeline.events.is_empty() {
        bail!("nutrition parser returned no meal events");
    }
    if timeline.events.len() > MAX_FUEL_EVENTS {
        bail!("nutrition parser returned too many meal events");
    }
    let item_count = timeline
        .events
        .iter()
        .map(|event| event.items.len())
        .sum::<usize>();
    if item_count == 0 {
        bail!("nutrition parser returned no food or drink items");
    }
    if item_count > MAX_BATCH_ITEMS {
        bail!("nutrition parser returned too many food or drink items");
    }

    let explicit_date = timeline
        .date
        .as_deref()
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .context("nutrition parser returned an invalid date")
        })
        .transpose()?;
    if explicit_date.is_some_and(|date| date > now.date_naive()) {
        bail!("future dates cannot be logged as fuel");
    }

    let explicit_times = timeline
        .events
        .iter()
        .filter_map(|event| event.time.as_deref())
        .map(parse_meal_time)
        .collect::<Result<Vec<_>>>()?;
    let inferred_yesterday = explicit_date.is_none()
        && now.time() < NaiveTime::from_hms_opt(YESTERDAY_INFERENCE_CUTOFF_HOUR, 0, 0).unwrap()
        && explicit_times.len() >= 2
        && explicit_times.iter().all(|time| *time > now.time());
    let date = match explicit_date {
        Some(date) => date,
        None if inferred_yesterday => now
            .date_naive()
            .checked_sub_days(Days::new(1))
            .context("determining yesterday")?,
        None => now.date_naive(),
    };

    let mut previous_time = None;
    let mut events = Vec::with_capacity(timeline.events.len());
    for (index, event) in timeline.events.into_iter().enumerate() {
        let explicit_time = event.time.as_deref().map(parse_meal_time).transpose()?;
        match (explicit_time, event.inherit_previous_time, index) {
            (Some(_), true, _) => {
                bail!("nutrition parser returned conflicting meal time fields")
            }
            (None, true, 0) => bail!("the first meal event cannot inherit a time"),
            (None, false, index) if index > 0 => {
                bail!("a later untimed meal event must inherit the previous time")
            }
            _ => {}
        }
        let time = explicit_time
            .or(previous_time)
            .unwrap_or_else(|| now.time());
        previous_time = Some(time);
        let source_text = event.source_text.trim();
        if source_text.is_empty() || source_text.chars().count() > MAX_EVENT_SOURCE_CHARS {
            bail!("nutrition parser returned an invalid meal description");
        }
        let local = Local
            .from_local_datetime(&date.and_time(time))
            .earliest()
            .context("the requested local meal time does not exist")?;
        events.push(TimedFuelEvent {
            consumed_at: local.with_timezone(&Utc),
            source_text: source_text.to_string(),
            parsed: FuelParseResult { items: event.items },
        });
    }
    events.sort_by_key(|event| event.consumed_at);
    validate_timed_events(&events)?;
    Ok((events, inferred_yesterday))
}

fn parse_meal_time(value: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .with_context(|| format!("nutrition parser returned an invalid meal time: {value}"))
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
    validate_aggregate_totals(&totals)
}

pub(crate) fn validate_timed_events(events: &[TimedFuelEvent]) -> Result<()> {
    if events.is_empty() {
        bail!("fuel batch contains no meal events");
    }
    if events.len() > MAX_FUEL_EVENTS {
        bail!("fuel batch contains too many meal events");
    }
    let item_count = events
        .iter()
        .map(|event| event.parsed.items.len())
        .sum::<usize>();
    if item_count > MAX_BATCH_ITEMS {
        bail!("fuel batch contains too many food or drink items");
    }
    let mut totals = NutritionTotals::default();
    for event in events {
        if event.source_text.trim().is_empty()
            || event.source_text.chars().count() > MAX_EVENT_SOURCE_CHARS
        {
            bail!("fuel batch contains an invalid meal description");
        }
        validate_parsed(&event.parsed)?;
        totals.add_assign(&event.parsed.totals());
    }
    validate_aggregate_totals(&totals)
}

fn validate_aggregate_totals(totals: &NutritionTotals) -> Result<()> {
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
    let item_schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "name", "quantity", "unit", "calories", "protein_g",
            "carbohydrates_g", "fat_g", "fiber_g", "sugar_g",
            "sodium_mg", "potassium_mg", "assumptions"
        ],
        "properties": {
            "name": { "type": "string" },
            "quantity": { "type": ["number", "null"], "minimum": 0 },
            "unit": { "type": ["string", "null"] },
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
                "items": { "type": "string" }
            }
        }
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["date", "multiple_dates", "events"],
        "properties": {
            "date": {
                "type": ["string", "null"],
                "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"
            },
            "multiple_dates": { "type": "boolean" },
            "events": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_FUEL_EVENTS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["time", "inherit_previous_time", "source_text", "items"],
                    "properties": {
                        "time": {
                            "type": ["string", "null"],
                            "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$"
                        },
                        "inherit_previous_time": { "type": "boolean" },
                        "source_text": { "type": "string" },
                        "items": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": MAX_FUEL_ITEMS,
                            "items": item_schema
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
    fn schema_is_strict_bounded_and_openai_compatible() {
        let schema = fuel_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["events"]["maxItems"], 12);
        assert_eq!(
            schema["properties"]["events"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(
            schema["properties"]["events"]["items"]["properties"]["items"]["maxItems"],
            20
        );
        let serialized = schema.to_string();
        assert!(!serialized.contains("minLength"));
        assert!(!serialized.contains("maxLength"));
        assert!(include_str!("../prompts/fuel_entry.j2").contains("no more than 40"));
        assert!(include_str!("../prompts/fuel_entry.j2")
            .contains("separate events even when they have the same time"));
        assert_eq!(
            schema["properties"]["events"]["items"]["properties"]["items"]["items"]
                ["additionalProperties"],
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
            local_now: "2026-08-16T01:00:00+04:00".into(),
            timezone: "+04".into(),
        })
        .unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["input"], "oatmeal");
        assert_eq!(value["local_now"], "2026-08-16T01:00:00+04:00");
        assert!(value.get("recent_entries").is_none());
    }

    fn timeline(date: Option<&str>, times: &[Option<&str>]) -> FuelTimelineParseResult {
        FuelTimelineParseResult {
            date: date.map(str::to_string),
            multiple_dates: false,
            events: times
                .iter()
                .enumerate()
                .map(|(index, time)| FuelTimelineEvent {
                    time: time.map(str::to_string),
                    inherit_previous_time: time.is_none() && index > 0,
                    source_text: format!("meal {}", index + 1),
                    items: parsed_item(&format!("item {index}")).items,
                })
                .collect(),
        }
    }

    fn local_now(hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 16, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn whole_day_with_only_future_times_rolls_back_to_yesterday() {
        let mut parsed = timeline(None, &[Some("10:00"), Some("14:00"), Some("19:00")]);
        parsed.events[0].items = ["sunny side up eggs", "peas"]
            .into_iter()
            .flat_map(|name| parsed_item(name).items)
            .collect();
        parsed.events[1].items = ["Lay's with cheese", "M&M's"]
            .into_iter()
            .flat_map(|name| parsed_item(name).items)
            .collect();
        parsed.events[2].items = ["beer", "Doritos chips"]
            .into_iter()
            .flat_map(|name| parsed_item(name).items)
            .collect();
        let (events, inferred) = resolve_timeline_at(parsed, local_now(1)).unwrap();

        assert!(inferred);
        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event.parsed.items.len())
                .sum::<usize>(),
            6
        );
        assert_eq!(events[0].parsed.items[0].name, "sunny side up eggs");
        assert_eq!(events[2].parsed.items[1].name, "Doritos chips");
        assert!(events.iter().all(|event| {
            event.consumed_at.with_timezone(&Local).date_naive()
                == NaiveDate::from_ymd_opt(2026, 8, 15).unwrap()
        }));
    }

    #[test]
    fn mixed_or_single_future_times_stay_today() {
        for (hour, times) in [
            (4, vec![Some("10:00"), Some("19:00")]),
            (8, vec![Some("10:00"), Some("14:00")]),
            (15, vec![Some("18:00"), Some("20:00")]),
            (15, vec![Some("19:00")]),
        ] {
            let (events, inferred) =
                resolve_timeline_at(timeline(None, &times), local_now(hour)).unwrap();
            assert!(!inferred);
            assert!(events.iter().all(|event| {
                event.consumed_at.with_timezone(&Local).date_naive()
                    == NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()
            }));
        }
    }

    #[test]
    fn explicit_dates_override_rollover_and_future_dates_are_rejected() {
        let (events, inferred) = resolve_timeline_at(
            timeline(Some("2026-08-16"), &[Some("10:00"), Some("19:00")]),
            local_now(1),
        )
        .unwrap();
        assert!(!inferred);
        assert_eq!(
            events[0].consumed_at.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()
        );

        let error =
            resolve_timeline_at(timeline(Some("2026-08-17"), &[Some("10:00")]), local_now(1))
                .unwrap_err();
        assert!(error.to_string().contains("future dates"));
    }

    #[test]
    fn untimed_and_equal_time_events_remain_separate_in_input_order() {
        let (events, _) = resolve_timeline_at(
            timeline(None, &[Some("10:00"), None, Some("10:00")]),
            local_now(12),
        )
        .unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event.source_text.as_str())
                .collect::<Vec<_>>(),
            vec!["meal 1", "meal 2", "meal 3"]
        );
        assert!(events.iter().all(|event| {
            event
                .consumed_at
                .with_timezone(&Local)
                .format("%H:%M")
                .to_string()
                == "10:00"
        }));
    }

    #[test]
    fn equal_time_repeated_foods_are_separate_and_all_counted() {
        let mut parsed = timeline(None, &[Some("11:00"), Some("11:00")]);
        parsed.events[0].source_text = "espresso with 90 ml milk".into();
        parsed.events[0].items[0].name = "milk".into();
        parsed.events[1].source_text = "protein shake with 500 ml milk".into();
        parsed.events[1].items[0].name = "milk".into();

        let (events, _) = resolve_timeline_at(parsed, local_now(12)).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].source_text, "espresso with 90 ml milk");
        assert_eq!(events[1].source_text, "protein shake with 500 ml milk");
        assert_eq!(
            events
                .iter()
                .map(|event| event.parsed.totals().calories)
                .sum::<f64>(),
            800.0
        );
    }

    #[test]
    fn contradictory_time_inheritance_is_rejected() {
        let mut explicit_inherit = timeline(None, &[Some("10:00")]);
        explicit_inherit.events[0].inherit_previous_time = true;
        assert!(resolve_timeline_at(explicit_inherit, local_now(12)).is_err());

        let mut first_inherit = timeline(None, &[None]);
        first_inherit.events[0].inherit_previous_time = true;
        assert!(resolve_timeline_at(first_inherit, local_now(12)).is_err());

        let mut later_without_inherit = timeline(None, &[Some("10:00"), None]);
        later_without_inherit.events[1].inherit_previous_time = false;
        assert!(resolve_timeline_at(later_without_inherit, local_now(12)).is_err());
    }

    #[test]
    fn multiple_calendar_dates_are_rejected() {
        let mut parsed = timeline(None, &[Some("10:00")]);
        parsed.multiple_dates = true;
        assert!(resolve_timeline_at(parsed, local_now(12))
            .unwrap_err()
            .to_string()
            .contains("one calendar date"));
    }

    #[test]
    fn validation_allows_repeated_items_and_rejects_other_invalid_fields() {
        let mut duplicate = parsed_item("Greek yogurt");
        let mut second = duplicate.items[0].clone();
        second.name = "  greek   YOGURT ".into();
        duplicate.items.push(second);
        validate_parsed(&duplicate).unwrap();
        assert_eq!(duplicate.totals().calories, 800.0);

        let mut blank_unit = parsed_item("tea");
        blank_unit.items[0].unit = Some("  ".into());
        assert!(validate_parsed(&blank_unit).is_err());

        let mut sugar = parsed_item("dessert");
        sugar.items[0].nutrition.sugar_g = 60.0;
        assert!(validate_parsed(&sugar).is_err());
    }

    #[test]
    fn validation_enforces_string_limits_outside_the_openai_schema() {
        let mut long_name = parsed_item(&"n".repeat(121));
        assert!(validate_parsed(&long_name).is_err());

        long_name.items[0].name = "tea".into();
        long_name.items[0].unit = Some("u".repeat(41));
        assert!(validate_parsed(&long_name).is_err());

        long_name.items[0].unit = None;
        long_name.items[0].assumptions = vec!["a".repeat(201)];
        assert!(validate_parsed(&long_name).is_err());

        let mut long_source = timeline(None, &[Some("10:00")]);
        long_source.events[0].source_text = "s".repeat(MAX_EVENT_SOURCE_CHARS + 1);
        assert!(resolve_timeline_at(long_source, local_now(12)).is_err());
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
