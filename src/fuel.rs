use crate::config::{Config, Paths, RecommenderBackend, UnitSystem};
use crate::models::{FuelItem, FuelParseResult, NutritionTotals, TimedFuelEvent};
use crate::prompt_templates::PromptRenderer;
use crate::recommender;
use crate::storage::Store;
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use chrono::{DateTime, Days, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const FUEL_MODEL: &str = "gpt-6-luna";
const MAX_FUEL_ITEMS: usize = 20;
const MAX_FUEL_EVENTS: usize = 12;
const MAX_BATCH_ITEMS: usize = 40;
const MAX_FUEL_ITEM_NAME_CHARS: usize = 500;
const MAX_EVENT_SOURCE_CHARS: usize = 500;
const MAX_SUGAR_CARB_ROUNDING_GAP_G: f64 = 1.0;
const YESTERDAY_INFERENCE_CUTOFF_HOUR: u32 = 4;
pub const MAX_FUEL_INPUT_CHARS: usize = 2_000;
pub const MAX_FUEL_IMAGES: usize = 10;
const MAX_FUEL_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// Upper bound for all prepared photos in one request; only reachable when no
/// conversion tool is installed and originals are sent unchanged.
const MAX_PREPARED_IMAGES_BYTES: u64 = 40 * 1024 * 1024;
const MAX_IMAGE_MEALS: usize = 12;
const MAX_IMAGE_INGREDIENTS: usize = 40;
const MAX_IMAGE_TIME_HINT_CHARS: usize = 100;
pub const FUEL_IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "gif", "heic", "heif"];
/// Photos are re-encoded as JPEGs no larger than this on the long side.
const MAX_IMAGE_SIDE_PX: u32 = 1080;
/// Reading several photos takes longer than parsing text, so the photo step
/// gets at least this long regardless of the recommender timeout.
const MIN_IMAGE_TIMEOUT_MS: u64 = 180_000;

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

#[derive(Debug, Deserialize)]
struct FuelImageDescription {
    meals: Vec<FuelImageMeal>,
    notes: String,
}

#[derive(Debug, Deserialize)]
struct FuelImageMeal {
    name: String,
    time_hint: String,
    ingredients: Vec<FuelImageIngredient>,
}

#[derive(Debug, Deserialize)]
struct FuelImageIngredient {
    name: String,
    grams: f64,
}

#[derive(Serialize)]
struct FuelImagesPromptContext<'a> {
    user_note: &'a str,
    image_count: usize,
}

/// Parses a meal from text, optionally reading attached photos first. Photos are
/// turned into a text description that goes through the regular text parser.
pub fn parse_fuel_with_images(
    store: &Store,
    config: &Config,
    paths: &Paths,
    input: &str,
    images: &[PathBuf],
    cancel: Arc<AtomicBool>,
) -> Result<FuelParseOutcome> {
    if images.is_empty() {
        return parse_fuel(store, config, paths, input, cancel);
    }
    let description = describe_fuel_images(store, config, paths, input, images, cancel.clone())?;
    if cancel.load(Ordering::Acquire) {
        bail!("meal parsing was cancelled");
    }
    // The note is already folded into the description: quantity hints are
    // applied to the grams and date or time words are kept per meal, so it is
    // not passed on again where "ate half" would be applied twice.
    let synthetic = synthetic_fuel_input(&description)?;
    parse_fuel(store, config, paths, &synthetic, cancel)
}

fn describe_fuel_images(
    store: &Store,
    config: &Config,
    paths: &Paths,
    note: &str,
    images: &[PathBuf],
    cancel: Arc<AtomicBool>,
) -> Result<FuelImageDescription> {
    if images.len() > MAX_FUEL_IMAGES {
        bail!("attach no more than {MAX_FUEL_IMAGES} photos per meal");
    }
    let note = note.trim();
    if note.chars().count() > MAX_FUEL_INPUT_CHARS {
        bail!("meal or drink description is limited to {MAX_FUEL_INPUT_CHARS} characters");
    }
    let context = FuelImagesPromptContext {
        user_note: note,
        image_count: images.len(),
    };
    let prompt = PromptRenderer::new(&paths.config_dir).fuel_images(&context)?;
    let schema = fuel_images_schema();
    let mut config = config.clone();
    config.recommender.timeout_ms = config.recommender.timeout_ms.max(MIN_IMAGE_TIMEOUT_MS);
    let staging = tempfile::tempdir().context("staging meal photos")?;
    let prepared = prepare_fuel_images(images, staging.path(), &cancel)?;
    let description: FuelImageDescription = match config.recommender.backend {
        RecommenderBackend::Codex => recommender::call_codex_json_for_model_with_images(
            store,
            &config,
            &prompt,
            &prepared,
            &schema,
            FUEL_MODEL,
            cancel.as_ref(),
        )
        .context("reading meal photos with Codex")?,
        RecommenderBackend::OpenaiEnv | RecommenderBackend::OpenaiKeyring => {
            let mut data_urls = Vec::with_capacity(prepared.len());
            for image in &prepared {
                data_urls.push(fuel_image_data_url(image)?);
            }
            let body = openai_fuel_images_request_body(&config, &prompt, &data_urls, schema);
            recommender::call_openai_json_cancellable(store, &config, paths, body, cancel)
                .context("reading meal photos with OpenAI")?
        }
        RecommenderBackend::Local => {
            bail!("reading meal photos needs Codex or an OpenAI backend")
        }
    };
    validate_image_description(&description)?;
    Ok(description)
}

/// Copies photos into `staging` under neutral names (paths with commas or odd
/// characters would otherwise confuse Codex's `--image` parsing), re-encoding
/// each as a JPEG of at most `MAX_IMAGE_SIDE_PX`. Without a conversion tool,
/// photos other than HEIC are sent as they are.
fn prepare_fuel_images(
    images: &[PathBuf],
    staging: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>> {
    let mut prepared = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            bail!("meal parsing was cancelled");
        }
        let extension = fuel_image_extension(image)?;
        check_fuel_image_size(image)?;
        let jpeg = staging.join(format!("photo-{}.jpg", index + 1));
        let heic = matches!(extension.as_str(), "heic" | "heif");
        match convert_to_jpeg(image, &jpeg) {
            Ok(true) => {
                prepared.push(jpeg);
                continue;
            }
            Ok(false) if heic => bail!(
                "{} is a HEIC photo; converting it needs sips (macOS) or ImageMagick or libheif (Linux)",
                image.display()
            ),
            Err(error) if heic => return Err(error),
            // The backend can still read the original PNG, JPEG, WebP, or GIF.
            Ok(false) | Err(_) => {}
        }
        let destination = staging.join(format!("photo-{}.{extension}", index + 1));
        fs::copy(image, &destination)
            .with_context(|| format!("reading photo {}", image.display()))?;
        prepared.push(destination);
    }
    let total = prepared
        .iter()
        .map(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        })
        .sum::<u64>();
    if total > MAX_PREPARED_IMAGES_BYTES {
        bail!(
            "these photos add up to more than {} MB; attach fewer, or install ImageMagick so Svarog can shrink them",
            MAX_PREPARED_IMAGES_BYTES / 1024 / 1024
        );
    }
    Ok(prepared)
}

/// Returns false when no conversion tool is installed.
fn convert_to_jpeg(input: &Path, output: &Path) -> Result<bool> {
    // `sips -Z` also enlarges, so only pass it for photos that need shrinking.
    let sips_downscale = cfg!(target_os = "macos")
        && sips_long_side(input).is_none_or(|side| side > MAX_IMAGE_SIDE_PX);
    for arguments in image_conversion_commands(input, output, sips_downscale) {
        let result = std::process::Command::new(&arguments[0])
            .args(&arguments[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .output();
        let result = match result {
            Ok(result) => result,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("running {}", arguments[0]));
            }
        };
        if !result.status.success() || !output.is_file() {
            bail!(
                "could not convert photo {} with {}: {}",
                input.display(),
                arguments[0],
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }
        return Ok(true);
    }
    Ok(false)
}

fn sips_long_side(input: &Path) -> Option<u32> {
    let output = std::process::Command::new("sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(input)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            matches!(key, "pixelWidth" | "pixelHeight")
                .then(|| value.trim().parse::<u32>().ok())
                .flatten()
        })
        .max()
}

fn image_conversion_commands(
    input: &Path,
    output: &Path,
    sips_downscale: bool,
) -> Vec<Vec<String>> {
    let input = input.display().to_string();
    let output = output.display().to_string();
    let max = MAX_IMAGE_SIDE_PX.to_string();
    let mut sips = vec!["sips".into(), "-s".into(), "format".into(), "jpeg".into()];
    sips.extend(["-s".into(), "formatOptions".into(), "85".into()]);
    if sips_downscale {
        sips.extend(["-Z".into(), max.clone()]);
    }
    sips.extend([input.clone(), "--out".into(), output.clone()]);
    let imagemagick = |binary: &str| -> Vec<String> {
        vec![
            binary.into(),
            input.clone(),
            "-auto-orient".into(),
            "-resize".into(),
            format!("{max}x{max}>"),
            "-quality".into(),
            "85".into(),
            format!("jpeg:{output}"),
        ]
    };
    let mut commands = Vec::new();
    if cfg!(target_os = "macos") {
        commands.push(sips);
    }
    commands.push(imagemagick("magick"));
    if !cfg!(target_os = "macos") {
        // On macOS `convert` is not ImageMagick's, so only try it elsewhere.
        commands.push(imagemagick("convert"));
    }
    commands.push(vec![
        "heif-convert".into(),
        "-q".into(),
        "85".into(),
        input,
        output,
    ]);
    commands
}

pub fn is_fuel_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            FUEL_IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn fuel_image_extension(path: &Path) -> Result<String> {
    if !is_fuel_image_path(path) {
        bail!(
            "{} is not a supported photo; use PNG, JPEG, HEIC, WebP, or GIF",
            path.display()
        );
    }
    Ok(path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase())
}

fn check_fuel_image_size(path: &Path) -> Result<u64> {
    let size = fs::metadata(path)
        .with_context(|| format!("reading photo {}", path.display()))?
        .len();
    if size > MAX_FUEL_IMAGE_BYTES {
        bail!(
            "{} is larger than {} MB",
            path.display(),
            MAX_FUEL_IMAGE_BYTES / 1024 / 1024
        );
    }
    Ok(size)
}

fn fuel_image_data_url(path: &Path) -> Result<String> {
    let mime = match fuel_image_extension(path)?.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        other => bail!("{other} photos must be converted before upload"),
    };
    check_fuel_image_size(path)?;
    let bytes = fs::read(path).with_context(|| format!("reading photo {}", path.display()))?;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn validate_image_description(description: &FuelImageDescription) -> Result<()> {
    if description.meals.is_empty() {
        let notes = description.notes.trim();
        if notes.is_empty() {
            bail!("no food or drink was recognized in the photos");
        }
        bail!("no food or drink was recognized in the photos: {notes}");
    }
    if description.meals.len() > MAX_IMAGE_MEALS {
        bail!("photo reader returned more than {MAX_IMAGE_MEALS} meals");
    }
    let ingredients: usize = description
        .meals
        .iter()
        .map(|meal| meal.ingredients.len())
        .sum();
    if ingredients > MAX_IMAGE_INGREDIENTS {
        bail!("photo reader returned more than {MAX_IMAGE_INGREDIENTS} ingredients");
    }
    for meal in &description.meals {
        if meal.name.trim().is_empty() {
            bail!("photo reader returned a meal without a name");
        }
        if meal.time_hint.chars().count() > MAX_IMAGE_TIME_HINT_CHARS {
            bail!(
                "photo reader returned an overly long time for {}",
                meal.name
            );
        }
        for ingredient in &meal.ingredients {
            if ingredient.name.trim().is_empty()
                || !ingredient.grams.is_finite()
                || ingredient.grams < 0.0
            {
                bail!(
                    "photo reader returned an invalid ingredient for {}",
                    meal.name
                );
            }
        }
    }
    Ok(())
}

/// Renders the photo description as a timeline the text parser already
/// understands, one line per meal.
fn synthetic_fuel_input(description: &FuelImageDescription) -> Result<String> {
    let meals = description
        .meals
        .iter()
        .map(|meal| {
            let ingredients = meal
                .ingredients
                .iter()
                .map(|ingredient| {
                    format!(
                        "{} {} g",
                        ingredient.name.trim(),
                        ingredient.grams.round().max(1.0)
                    )
                })
                .collect::<Vec<_>>();
            let food = if ingredients.is_empty() {
                meal.name.trim().to_string()
            } else {
                format!("{} ({})", meal.name.trim(), ingredients.join(", "))
            };
            match meal.time_hint.trim() {
                "" => food,
                time => format!("{time} - {food}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "Meals read from photos. Gram amounts are the final totals eaten, already combined across photos.\n{meals}"
    );
    let notes = description.notes.trim();
    let with_notes = format!("{text}\nPhoto notes (context only, not additional food): {notes}");
    if !notes.is_empty() && with_notes.chars().count() <= MAX_FUEL_INPUT_CHARS {
        return Ok(with_notes);
    }
    if text.chars().count() > MAX_FUEL_INPUT_CHARS {
        bail!(
            "the photos describe more food than fits in one entry; try fewer photos or split the meal"
        );
    }
    Ok(text)
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
        let mut parsed = FuelParseResult { items: event.items };
        normalize_model_nutrition(&mut parsed);
        events.push(TimedFuelEvent {
            consumed_at: local.with_timezone(&Utc),
            source_text: source_text.to_string(),
            parsed,
        });
    }
    events.sort_by_key(|event| event.consumed_at);
    validate_timed_events(&events)?;
    Ok((events, inferred_yesterday))
}

fn normalize_model_nutrition(parsed: &mut FuelParseResult) {
    for item in &mut parsed.items {
        let sugar_carb_gap = item.nutrition.sugar_g - item.nutrition.carbohydrates_g;
        if sugar_carb_gap > 0.0 && sugar_carb_gap <= MAX_SUGAR_CARB_ROUNDING_GAP_G {
            item.nutrition.sugar_g = item.nutrition.carbohydrates_g;
        }
    }
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

fn openai_fuel_images_request_body(
    config: &Config,
    prompt: &str,
    data_urls: &[String],
    schema: serde_json::Value,
) -> serde_json::Value {
    let mut content = vec![json!({ "type": "input_text", "text": prompt })];
    content.extend(
        data_urls
            .iter()
            .map(|url| json!({ "type": "input_image", "image_url": url, "detail": "auto" })),
    );
    json!({
        "model": FUEL_MODEL,
        "reasoning": { "effort": config.recommender.openai.reasoning_effort },
        "input": [{ "role": "user", "content": content }],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "svarog_fuel_images",
                "strict": true,
                "schema": schema
            }
        }
    })
}

fn fuel_images_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["meals", "notes"],
        "properties": {
            "meals": {
                "type": "array",
                "maxItems": MAX_IMAGE_MEALS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "time_hint", "ingredients"],
                    "properties": {
                        "name": { "type": "string" },
                        "time_hint": { "type": "string" },
                        "ingredients": {
                            "type": "array",
                            "maxItems": MAX_FUEL_ITEMS,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["name", "grams"],
                                "properties": {
                                    "name": { "type": "string" },
                                    "grams": { "type": "number" }
                                }
                            }
                        }
                    }
                }
            },
            "notes": { "type": "string" }
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
        if item.name.trim().is_empty() || item.name.chars().count() > MAX_FUEL_ITEM_NAME_CHARS {
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
        assert!(include_str!("../prompts/fuel_entry.j2")
            .contains("inherit_previous_time=false whenever `time` is not null"));
        assert!(include_str!("../prompts/fuel_entry.j2")
            .contains("Sugar is part of total carbohydrates"));
        assert!(include_str!("../prompts/fuel_entry.j2").contains("no more than 120 characters"));
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

    fn image_description(meals: &[(&str, &[(&str, f64)])], notes: &str) -> FuelImageDescription {
        FuelImageDescription {
            meals: meals
                .iter()
                .map(|(name, ingredients)| FuelImageMeal {
                    name: (*name).into(),
                    time_hint: String::new(),
                    ingredients: ingredients
                        .iter()
                        .map(|(name, grams)| FuelImageIngredient {
                            name: (*name).into(),
                            grams: *grams,
                        })
                        .collect(),
                })
                .collect(),
            notes: notes.into(),
        }
    }

    #[test]
    fn image_schema_is_strict_and_bounded() {
        let schema = fuel_images_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["meals"]["maxItems"], 12);
        let meal = &schema["properties"]["meals"]["items"];
        assert_eq!(meal["additionalProperties"], false);
        assert_eq!(
            meal["properties"]["ingredients"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(
            meal["properties"]["ingredients"]["items"]["required"],
            json!(["name", "grams"])
        );
        assert_eq!(
            meal["required"],
            json!(["name", "time_hint", "ingredients"])
        );
    }

    #[test]
    fn openai_image_payload_sends_every_photo_to_luna() {
        let config = Config::default();
        let urls = vec![
            "data:image/png;base64,AAAA".to_string(),
            "data:image/jpeg;base64,BBBB".to_string(),
        ];
        let body = openai_fuel_images_request_body(&config, "photos", &urls, fuel_images_schema());

        assert_eq!(body["model"], FUEL_MODEL);
        assert_eq!(body["text"]["format"]["strict"], true);
        let content = body["input"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "photos");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], urls[0]);
        assert_eq!(content[2]["image_url"], urls[1]);
    }

    #[test]
    fn image_data_url_uses_the_photo_type_and_rejects_other_files() {
        let root = tempfile::tempdir().unwrap();
        let photo = root.path().join("Lunch.JPG");
        fs::write(&photo, [1u8, 2, 3]).unwrap();
        assert_eq!(
            fuel_image_data_url(&photo).unwrap(),
            "data:image/jpeg;base64,AQID"
        );
        let text = root.path().join("notes.txt");
        fs::write(&text, "x").unwrap();
        assert!(fuel_image_data_url(&text).is_err());
        assert!(fuel_image_data_url(&root.path().join("missing.png")).is_err());
    }

    const TINY_PNG: [u8; 73] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 4, 8, 2,
        0, 0, 0, 38, 147, 9, 41, 0, 0, 0, 16, 73, 68, 65, 84, 120, 156, 99, 248, 223, 192, 0, 71,
        12, 196, 113, 0, 126, 162, 23, 241, 93, 161, 96, 5, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
        96, 130,
    ];

    #[test]
    fn photos_are_staged_under_neutral_names() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let photo = root.path().join("plate, top.png");
        fs::write(&photo, TINY_PNG).unwrap();

        let prepared =
            prepare_fuel_images(&[photo], staging.path(), &AtomicBool::new(false)).unwrap();

        assert_eq!(prepared.len(), 1);
        assert!(prepared[0].starts_with(staging.path()));
        let name = prepared[0].file_name().unwrap().to_str().unwrap();
        assert!(name == "photo-1.jpg" || name == "photo-1.png", "{name}");
        assert!(fuel_image_data_url(&prepared[0]).is_ok());
    }

    #[test]
    fn photo_preparation_stops_when_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let photo = root.path().join("plate.png");
        fs::write(&photo, TINY_PNG).unwrap();

        let error = prepare_fuel_images(&[photo], root.path(), &AtomicBool::new(true)).unwrap_err();

        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn conversion_caps_the_long_side_without_enlarging() {
        let input = Path::new("/in.heic");
        let output = Path::new("/out.jpg");
        let small = image_conversion_commands(input, output, false);
        let large = image_conversion_commands(input, output, true);
        let magick = |commands: &[Vec<String>]| {
            commands
                .iter()
                .find(|command| command[0] == "magick")
                .cloned()
                .unwrap()
        };
        assert!(magick(&small).contains(&"1080x1080>".to_string()));
        assert_eq!(magick(&large).last().unwrap(), "jpeg:/out.jpg");
        assert_eq!(small.last().unwrap()[0], "heif-convert");
        if cfg!(target_os = "macos") {
            assert_eq!(small[0][0], "sips");
            assert!(!small[0].contains(&"-Z".to_string()));
            assert!(large[0].contains(&"-Z".to_string()));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn photos_become_jpegs_capped_at_1080_px() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let png = root.path().join("source.png");
        let heic = root.path().join("IMG_0001.HEIC");
        fs::write(&png, TINY_PNG).unwrap();
        let status = std::process::Command::new("sips")
            .args(["-s", "format", "heic"])
            .arg(&png)
            .arg("--out")
            .arg(&heic)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let large = root.path().join("large.png");
        let status = std::process::Command::new("sips")
            .args(["-z", "1500", "2400"])
            .arg(&png)
            .arg("--out")
            .arg(&large)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let prepared =
            prepare_fuel_images(&[heic, large], staging.path(), &AtomicBool::new(false)).unwrap();

        assert_eq!(
            prepared,
            vec![
                staging.path().join("photo-1.jpg"),
                staging.path().join("photo-2.jpg")
            ]
        );
        assert!(fuel_image_data_url(&prepared[0])
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        assert_eq!(sips_long_side(&prepared[0]), Some(4));
        assert_eq!(sips_long_side(&prepared[1]), Some(1080));
    }

    #[test]
    fn synthetic_input_is_a_timeline_of_final_amounts() {
        let mut description = image_description(
            &[
                ("Caesar salad", &[("romaine", 80.4), ("chicken", 120.0)]),
                ("Latte", &[("whole milk", 250.0)]),
            ],
            "dressing amount is a guess",
        );
        description.meals[0].time_hint = " yesterday lunch ".into();

        let text = synthetic_fuel_input(&description).unwrap();

        assert_eq!(
            text,
            "Meals read from photos. Gram amounts are the final totals eaten, already combined across photos.\n\
             yesterday lunch - Caesar salad (romaine 80 g, chicken 120 g)\n\
             Latte (whole milk 250 g)\n\
             Photo notes (context only, not additional food): dressing amount is a guess"
        );
        let apple = synthetic_fuel_input(&image_description(&[("Apple", &[])], "")).unwrap();
        assert!(apple.ends_with("photos.\nApple"));
    }

    #[test]
    fn synthetic_input_drops_notes_then_fails_when_too_long() {
        let long_name = "x".repeat(MAX_FUEL_INPUT_CHARS - 40);
        let ingredients = [(long_name.as_str(), 10.0)];
        let fits = image_description(
            &[("Bowl", &ingredients[..0])],
            &"n".repeat(MAX_FUEL_INPUT_CHARS),
        );
        assert!(!synthetic_fuel_input(&fits).unwrap().contains("Photo notes"));
        let too_long = image_description(&[("Bowl", &ingredients)], "");
        assert!(synthetic_fuel_input(&too_long).is_err());
    }

    #[test]
    fn image_description_needs_food() {
        let empty = image_description(&[], "a photo of a cat");
        let error = validate_image_description(&empty).unwrap_err();
        assert!(error.to_string().contains("a photo of a cat"));
        let negative = image_description(&[("Soup", &[("broth", -1.0)])], "");
        assert!(validate_image_description(&negative).is_err());
        let fine = image_description(&[("Soup", &[("broth", 300.0)])], "");
        assert!(validate_image_description(&fine).is_ok());
    }

    #[test]
    fn photos_are_read_then_parsed_as_text_through_codex() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("fake-codex.sh");
        let log = root.path().join("prompts.log");
        let script = r#"#!/bin/sh
images=0
prompt=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --image) shift; images=$((images + 1)) ;;
    *) prompt="$1" ;;
  esac
  shift
done
if [ "$images" -gt 0 ]; then
  printf 'images %s\n' "$images" >> "__LOG__"
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"meals\":[{\"name\":\"Omelette\",\"time_hint\":\"8am\",\"ingredients\":[{\"name\":\"egg\",\"grams\":100}]}],\"notes\":\"\"}"}}'
else
  case "$prompt" in
    *"ate half"*) printf 'note repeated\n' >> "__LOG__" ;;
    *"8am - Omelette (egg 100 g)"*) printf 'text ok\n' >> "__LOG__" ;;
    *) printf 'text missing\n' >> "__LOG__" ;;
  esac
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"date\":null,\"multiple_dates\":false,\"events\":[{\"time\":\"08:00\",\"inherit_previous_time\":false,\"source_text\":\"omelette, egg 100 g\",\"items\":[{\"name\":\"Omelette\",\"quantity\":100,\"unit\":\"g\",\"calories\":150,\"protein_g\":12,\"carbohydrates_g\":1,\"fat_g\":10,\"fiber_g\":0,\"sugar_g\":1,\"sodium_mg\":140,\"potassium_mg\":130,\"assumptions\":[]}]}]}"}}'
fi
"#
        .replace("__LOG__", &log.display().to_string());
        fs::write(&command, script).unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        let photo = root.path().join("plate, top.png");
        fs::write(&photo, TINY_PNG).unwrap();
        let mut config = Config::default();
        config.recommender.backend = RecommenderBackend::Codex;
        config.recommender.codex.command = command.display().to_string();
        config.recommender.codex.args.clear();
        config.recommender.timeout_ms = 5_000;
        let paths = Paths::from_root(root.path().join("svarog"));
        let store = Store::open(&root.path().join("test.sqlite3")).unwrap();

        let outcome = parse_fuel_with_images(
            &store,
            &config,
            &paths,
            "breakfast at 8am, ate half",
            &[photo.clone(), photo],
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(log).unwrap(), "images 2\ntext ok\n");
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].parsed.items[0].name, "Omelette");
        assert_eq!(outcome.model, FUEL_MODEL);
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
    fn invalid_untimed_inheritance_is_rejected() {
        let mut first_inherit = timeline(None, &[None]);
        first_inherit.events[0].inherit_previous_time = true;
        assert!(resolve_timeline_at(first_inherit, local_now(12)).is_err());

        let mut later_without_inherit = timeline(None, &[Some("10:00"), None]);
        later_without_inherit.events[1].inherit_previous_time = false;
        assert!(resolve_timeline_at(later_without_inherit, local_now(12)).is_err());
    }

    #[test]
    fn explicit_times_override_inheritance_flags_for_repeated_times() {
        let mut parsed = timeline(
            None,
            &[
                Some("12:00"),
                Some("14:00"),
                Some("14:00"),
                Some("16:00"),
                Some("17:00"),
                Some("17:00"),
                Some("20:00"),
            ],
        );
        parsed.events[2].inherit_previous_time = true;
        parsed.events[5].inherit_previous_time = true;

        let (events, _) = resolve_timeline_at(parsed, local_now(21)).unwrap();

        assert_eq!(events.len(), 7);
        assert_eq!(
            events
                .iter()
                .map(|event| event.source_text.as_str())
                .collect::<Vec<_>>(),
            vec!["meal 1", "meal 2", "meal 3", "meal 4", "meal 5", "meal 6", "meal 7"]
        );
        assert_eq!(events[2].consumed_at, events[1].consumed_at);
        assert_eq!(events[5].consumed_at, events[4].consumed_at);
        assert_eq!(
            events
                .iter()
                .map(|event| event.parsed.totals().calories)
                .sum::<f64>(),
            2_800.0
        );
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
    fn model_nutrition_normalizes_only_small_sugar_carb_rounding_gaps() {
        let mut shake = timeline(None, &[Some("12:00")]);
        shake.events[0].source_text =
            "protein shake with 500 ml milk and 80 g protein powder".into();
        shake.events[0].items = vec![
            FuelItem {
                name: "milk".into(),
                quantity: Some(500.0),
                unit: Some("ml".into()),
                nutrition: NutritionTotals {
                    calories: 305.0,
                    protein_g: 16.0,
                    carbohydrates_g: 24.0,
                    fat_g: 16.0,
                    fiber_g: 0.0,
                    sugar_g: 25.0,
                    sodium_mg: 215.0,
                    potassium_mg: 750.0,
                },
                assumptions: Vec::new(),
            },
            FuelItem {
                name: "protein powder".into(),
                quantity: Some(80.0),
                unit: Some("g".into()),
                nutrition: NutritionTotals {
                    calories: 320.0,
                    protein_g: 64.0,
                    carbohydrates_g: 8.0,
                    fat_g: 5.0,
                    fiber_g: 0.0,
                    sugar_g: 3.0,
                    sodium_mg: 300.0,
                    potassium_mg: 200.0,
                },
                assumptions: Vec::new(),
            },
        ];

        let (events, _) = resolve_timeline_at(shake, local_now(13)).unwrap();
        assert_eq!(events[0].parsed.items[0].nutrition.sugar_g, 24.0);
        assert_eq!(events[0].parsed.items[0].nutrition.carbohydrates_g, 24.0);
        assert_eq!(events[0].parsed.items[1].nutrition.sugar_g, 3.0);

        let mut invalid = timeline(None, &[Some("12:00")]);
        invalid.events[0].items[0].nutrition.carbohydrates_g = 23.0;
        invalid.events[0].items[0].nutrition.sugar_g = 25.0;
        assert!(resolve_timeline_at(invalid, local_now(13))
            .unwrap_err()
            .to_string()
            .contains("implausible nutrition values"));
    }

    #[test]
    fn validation_enforces_string_limits_outside_the_openai_schema() {
        let reported_burger_name = "Burger with beef chop, scrambled eggs, pickles, tomatoes, lettuce, cocktail sauce, crunch sauce, and grilled pepper sauce";
        assert_eq!(reported_burger_name.chars().count(), 121);
        validate_parsed(&parsed_item(reported_burger_name)).unwrap();

        validate_parsed(&parsed_item(&"n".repeat(MAX_FUEL_ITEM_NAME_CHARS))).unwrap();

        let mut long_name = parsed_item(&"n".repeat(MAX_FUEL_ITEM_NAME_CHARS + 1));
        assert!(validate_parsed(&long_name).is_err());

        long_name.items[0].name = "  ".into();
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
