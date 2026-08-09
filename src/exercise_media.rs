use crate::exercise_catalog::{ExerciseCatalogEntry, CATALOG_REVISION};
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const IMAGE_BASE_URL: &str = "https://raw.githubusercontent.com/yuhonas/free-exercise-db/";
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug)]
pub struct PreparedGallery {
    pub path: PathBuf,
}

pub fn prepare_gallery(data_dir: &Path, entry: &ExerciseCatalogEntry) -> Result<PreparedGallery> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("svarog/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building exercise image client")?;
    prepare_gallery_with(data_dir, entry, |url, destination| {
        download_image(&client, url, destination)
    })
}

fn prepare_gallery_with<F>(
    data_dir: &Path,
    entry: &ExerciseCatalogEntry,
    mut fetch: F,
) -> Result<PreparedGallery>
where
    F: FnMut(&str, &Path) -> Result<()>,
{
    if entry.images.is_empty() {
        bail!("No reference images are available for this exercise.");
    }
    validate_single_component(&entry.id).context("invalid exercise id")?;

    let cache_dir = data_dir
        .join("exercise-media")
        .join(CATALOG_REVISION)
        .join(&entry.id);
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating image cache at {}", cache_dir.display()))?;

    let mut local_images = Vec::with_capacity(entry.images.len());
    for (index, remote_path) in entry.images.iter().enumerate() {
        let url = remote_image_url(remote_path)?;
        let extension = image_extension(remote_path)?;
        let destination = cache_dir.join(format!("image-{}.{}", index + 1, extension));
        if !cached_image_is_valid(&destination)? {
            fetch(&url, &destination)
                .with_context(|| format!("downloading image {}", index + 1))?;
            if !cached_image_is_valid(&destination)? {
                bail!("downloaded image {} is not a supported image", index + 1);
            }
        }
        local_images.push(destination);
    }

    let gallery_path = cache_dir.join("index.html");
    write_gallery(&gallery_path, entry, &local_images)?;
    Ok(PreparedGallery { path: gallery_path })
}

fn download_image(client: &Client, url: &str, destination: &Path) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("requesting {url}"))?;
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    validate_image_response(&content_type, response.content_length(), None)?;

    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading image response")?;
    validate_image_response(&content_type, response.content_length(), Some(&bytes))?;

    let temporary = destination.with_extension("part");
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, destination)
        .with_context(|| format!("saving {}", destination.display()))?;
    Ok(())
}

fn remote_image_url(remote_path: &str) -> Result<String> {
    validate_relative_path(remote_path)?;
    let base = format!("{IMAGE_BASE_URL}{CATALOG_REVISION}/exercises/");
    Ok(reqwest::Url::parse(&base)
        .context("parsing exercise image base URL")?
        .join(remote_path)
        .context("building exercise image URL")?
        .to_string())
}

fn validate_single_component(value: &str) -> Result<()> {
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("expected one safe path component");
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe exercise image path: {value}");
    }
    Ok(())
}

fn validate_image_response(
    content_type: &str,
    declared_length: Option<u64>,
    bytes: Option<&[u8]>,
) -> Result<()> {
    if !content_type.starts_with("image/") {
        bail!("server returned {content_type:?} instead of an image");
    }
    if declared_length.is_some_and(|length| length > MAX_IMAGE_BYTES)
        || bytes.is_some_and(|value| value.len() as u64 > MAX_IMAGE_BYTES)
    {
        bail!("image exceeds the 10 MB download limit");
    }
    if bytes.is_some_and(|value| !supported_image_bytes(value)) {
        bail!("server returned invalid image data");
    }
    Ok(())
}

fn image_extension(remote_path: &str) -> Result<&str> {
    match Path::new(remote_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg") => Ok("jpg"),
        Some("jpeg") => Ok("jpeg"),
        Some("png") => Ok("png"),
        Some("webp") => Ok("webp"),
        _ => bail!("unsupported exercise image type: {remote_path}"),
    }
}

fn cached_image_is_valid(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(bytes.len() as u64 <= MAX_IMAGE_BYTES && supported_image_bytes(&bytes))
}

fn supported_image_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
}

fn write_gallery(
    gallery_path: &Path,
    entry: &ExerciseCatalogEntry,
    local_images: &[PathBuf],
) -> Result<()> {
    let title = html_escape(&crate::exercise_catalog::display_name(&entry.id));
    let instructions = if entry.instructions.is_empty() {
        "<p>No written instructions are available for this exercise.</p>".to_string()
    } else {
        let items = entry
            .instructions
            .iter()
            .map(|instruction| format!("<li>{}</li>", html_escape(instruction)))
            .collect::<String>();
        format!("<ol>{items}</ol>")
    };
    let images = local_images
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let filename = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            format!(
                "<figure><img src=\"{}\" alt=\"{} reference image {}\"><figcaption>Position {}</figcaption></figure>",
                html_escape(filename),
                title,
                index + 1,
                index + 1
            )
        })
        .collect::<String>();
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title} · Svarog</title><style>body{{font:17px system-ui,sans-serif;line-height:1.5;max-width:1100px;margin:0 auto;padding:32px;color:#e6e6e6;background:#070808}}h1{{color:#ff8c00}}ol{{padding-left:1.5rem}}.images{{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:24px}}figure{{margin:0}}img{{display:block;width:100%;height:auto;border-radius:8px}}figcaption{{padding-top:8px;color:#888}}</style></head><body><h1>{title}</h1>{instructions}<div class=\"images\">{images}</div></body></html>"
    );
    let temporary = gallery_path.with_extension("part");
    fs::write(&temporary, html).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, gallery_path)
        .with_context(|| format!("saving {}", gallery_path.display()))?;
    Ok(())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn open_gallery(path: &Path) -> Result<()> {
    let command = opener_command().context("no supported browser opener on this platform")?;
    Command::new(command)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("opening {} with {command}", path.display()))?;
    Ok(())
}

fn opener_command() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        Some("open")
    }
    #[cfg(target_os = "linux")]
    {
        Some("xdg-open")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use tempfile::tempdir;

    fn entry() -> ExerciseCatalogEntry {
        ExerciseCatalogEntry {
            id: "Goblet_Squat".into(),
            force: Some("push".into()),
            mechanic: Some("compound".into()),
            equipment: Some("kettlebells".into()),
            primary_muscles: vec!["quadriceps".into()],
            secondary_muscles: vec!["glutes".into()],
            category: "strength".into(),
            instructions: vec!["Hold <close> & squat.".into()],
            images: vec!["Goblet_Squat/0.jpg".into(), "Goblet_Squat/1.jpg".into()],
        }
    }

    #[test]
    fn image_urls_are_pinned_and_reject_unsafe_paths() {
        assert_eq!(
            remote_image_url("Goblet_Squat/0.jpg").unwrap(),
            format!("{IMAGE_BASE_URL}{CATALOG_REVISION}/exercises/Goblet_Squat/0.jpg")
        );
        assert!(remote_image_url("../secret.jpg").is_err());
        assert!(remote_image_url("..\\secret.jpg").is_err());
        assert!(remote_image_url("/tmp/image.jpg").is_err());
    }

    #[test]
    fn gallery_downloads_missing_images_and_reuses_valid_cache() {
        let root = tempdir().unwrap();
        let fetched = RefCell::new(Vec::new());
        let first = prepare_gallery_with(root.path(), &entry(), |url, destination| {
            fetched.borrow_mut().push(url.to_string());
            fs::write(destination, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
            Ok(())
        })
        .unwrap();

        assert_eq!(fetched.borrow().len(), 2);
        let html = fs::read_to_string(&first.path).unwrap();
        assert!(html.contains("Goblet Squat"));
        assert!(html.contains("Hold &lt;close&gt; &amp; squat."));
        assert!(html.contains("image-1.jpg"));
        assert!(html.contains("image-2.jpg"));

        let second_fetches = RefCell::new(0);
        let second = prepare_gallery_with(root.path(), &entry(), |_, _| {
            *second_fetches.borrow_mut() += 1;
            bail!("cache should have been reused")
        })
        .unwrap();
        assert_eq!(*second_fetches.borrow(), 0);
        assert_eq!(first.path, second.path);
    }

    #[test]
    fn invalid_cached_image_is_fetched_again() {
        let root = tempdir().unwrap();
        let cache = root
            .path()
            .join("exercise-media")
            .join(CATALOG_REVISION)
            .join("Goblet_Squat");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("image-1.jpg"), b"not an image").unwrap();
        let mut fetches = 0;

        prepare_gallery_with(root.path(), &entry(), |_, destination| {
            fetches += 1;
            fs::write(destination, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
            Ok(())
        })
        .unwrap();

        assert_eq!(fetches, 2);
    }

    #[test]
    fn image_signatures_are_validated() {
        assert!(supported_image_bytes(&[0xff, 0xd8, 0xff, 0xd9]));
        assert!(supported_image_bytes(b"\x89PNG\r\n\x1a\nrest"));
        assert!(supported_image_bytes(b"RIFFsizeWEBPrest"));
        assert!(!supported_image_bytes(b"<html>not an image</html>"));
        assert!(
            validate_image_response("text/html", None, Some(b"<html>oops</html>"))
                .unwrap_err()
                .to_string()
                .contains("instead of an image")
        );
        assert!(
            validate_image_response("image/jpeg", Some(MAX_IMAGE_BYTES + 1), None)
                .unwrap_err()
                .to_string()
                .contains("10 MB")
        );
        assert!(
            validate_image_response("image/jpeg", None, Some(b"not an image"))
                .unwrap_err()
                .to_string()
                .contains("invalid image data")
        );
    }

    #[test]
    fn supported_platform_has_an_opener() {
        assert!(opener_command().is_some());
    }
}
