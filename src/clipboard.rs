use crate::fuel;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempPath;

/// An image taken from the clipboard: either raw image data saved to a
/// temporary file, or image files copied in a file manager.
#[derive(Debug)]
pub enum ClipboardImage {
    Temp(TempPath),
    Files(Vec<PathBuf>),
    /// Copied files that include something other than a supported photo.
    NotPhoto(PathBuf),
}

/// What a bracketed paste turned out to be.
#[derive(Debug, PartialEq)]
pub enum PastedPaths {
    /// Ordinary text; insert it as typed input.
    NotPaths,
    Images(Vec<PathBuf>),
    Unsupported(PathBuf),
}

const MACOS_PNG_SCRIPT: &str = r#"on run argv
set imageData to (the clipboard as «class PNGf»)
set target to open for access (POSIX file (item 1 of argv)) with write permission
set eof of target to 0
write imageData to target
close access target
end run"#;

// Lists every file copied in Finder, one path per line.
const MACOS_FILES_SCRIPT: &str = r#"ObjC.import('AppKit');
function run() {
  const options = $.NSDictionary.dictionaryWithObjectForKey(true, $.NSPasteboardURLReadingFileURLsOnlyKey);
  const urls = $.NSPasteboard.generalPasteboard.readObjectsForClassesOptions($([$.NSURL]), options);
  const paths = [];
  for (let i = 0; i < urls.count; i++) paths.push(urls.objectAtIndex(i).path.js);
  return paths.join('\n');
}"#;

pub fn read_image() -> Result<Option<ClipboardImage>> {
    if cfg!(target_os = "macos") {
        read_macos_image()
    } else {
        read_linux_image()
    }
}

fn read_macos_image() -> Result<Option<ClipboardImage>> {
    // Finder puts the copied file's icon on the clipboard as image data too, so
    // copied files must be checked before image data.
    let output = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", MACOS_FILES_SCRIPT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("running osascript to read the clipboard")?;
    if output.status.success() {
        let copied = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if !copied.is_empty() {
            return Ok(Some(match classify_paths(copied) {
                PastedPaths::Images(paths) => ClipboardImage::Files(paths),
                PastedPaths::Unsupported(path) => ClipboardImage::NotPhoto(path),
                PastedPaths::NotPaths => return Ok(None),
            }));
        }
    }
    let path = new_temp_png()?;
    let status = Command::new("osascript")
        .arg("-e")
        .arg(MACOS_PNG_SCRIPT)
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running osascript to read the clipboard")?;
    if status.success() && fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 0) {
        return Ok(Some(ClipboardImage::Temp(path)));
    }
    Ok(None)
}

fn read_linux_image() -> Result<Option<ClipboardImage>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (list, read) = linux_commands(wayland);
    let types = match Command::new(list[0])
        .args(&list[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "reading clipboard images needs {}",
                if wayland {
                    "wl-paste (wl-clipboard)"
                } else {
                    "xclip"
                }
            )
        }
        Err(error) => return Err(error).context("listing clipboard types"),
    };
    if !String::from_utf8_lossy(&types.stdout)
        .lines()
        .any(|line| line.trim() == "image/png")
    {
        return Ok(None);
    }
    let output = Command::new(read[0])
        .args(&read[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("reading the clipboard image")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    let path = new_temp_png()?;
    fs::write(&path, output.stdout).context("saving the clipboard image")?;
    Ok(Some(ClipboardImage::Temp(path)))
}

fn linux_commands(wayland: bool) -> (&'static [&'static str], &'static [&'static str]) {
    if wayland {
        (
            &["wl-paste", "--list-types"],
            &["wl-paste", "--no-newline", "--type", "image/png"],
        )
    } else {
        (
            &["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"],
            &["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
        )
    }
}

fn new_temp_png() -> Result<TempPath> {
    Ok(tempfile::Builder::new()
        .prefix("svarog-meal-")
        .suffix(".png")
        .tempfile()
        .context("creating a file for the clipboard image")?
        .into_temp_path())
}

/// Recognizes pasted or drag-and-dropped file paths. Terminals paste dropped
/// files as shell-escaped or quoted paths separated by spaces or newlines.
pub fn pasted_paths(value: &str) -> PastedPaths {
    let Some(words) = shell_words(value) else {
        return PastedPaths::NotPaths;
    };
    if words.is_empty() {
        return PastedPaths::NotPaths;
    }
    classify_paths(words.iter().map(|word| path_from_word(word)).collect())
}

/// Sorts existing files into photos or the first unsupported file. Anything
/// that is not an existing absolute file path is treated as text.
fn classify_paths(paths: Vec<PathBuf>) -> PastedPaths {
    if paths.is_empty()
        || paths
            .iter()
            .any(|path| !path.is_absolute() || !path.is_file())
    {
        return PastedPaths::NotPaths;
    }
    match paths.iter().find(|path| !fuel::is_fuel_image_path(path)) {
        Some(other) => PastedPaths::Unsupported(other.clone()),
        None => PastedPaths::Images(paths),
    }
}

fn path_from_word(word: &str) -> PathBuf {
    if let Some(rest) = word.strip_prefix("file://") {
        return PathBuf::from(percent_decode(rest));
    }
    if let Some(rest) = word.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(word)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Splits on unquoted whitespace, honoring single quotes, double quotes, and
/// backslash escapes. Returns None for unbalanced quotes.
fn shell_words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => current.push(other),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => current.push(chars.next()?),
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_word = true;
                current.push(chars.next()?);
            }
            whitespace if whitespace.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                current.push(other);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_uses_wl_paste_on_wayland_and_xclip_otherwise() {
        assert_eq!(linux_commands(true).0[0], "wl-paste");
        assert!(linux_commands(true).1.contains(&"image/png"));
        assert_eq!(linux_commands(false).0[0], "xclip");
        assert!(linux_commands(false).1.contains(&"image/png"));
    }

    #[test]
    fn shell_words_handle_escapes_and_quotes() {
        assert_eq!(
            shell_words(r#"/a/my\ lunch.png '/b/c d.jpg' "/e/f.webp""#).unwrap(),
            vec!["/a/my lunch.png", "/b/c d.jpg", "/e/f.webp"]
        );
        assert_eq!(shell_words("'unclosed"), None);
    }

    #[test]
    fn pasted_paths_recognizes_dropped_images_only() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("my lunch.png");
        let second = root.path().join("dinner.JPG");
        let notes = root.path().join("notes.txt");
        for path in [&first, &second, &notes] {
            fs::write(path, "x").unwrap();
        }
        let escaped = first.display().to_string().replace(' ', "\\ ");
        let dropped = format!("{escaped}\n'{}'", second.display());
        assert_eq!(
            pasted_paths(&dropped),
            PastedPaths::Images(vec![first.clone(), second])
        );
        let url = format!("file://{}", first.display().to_string().replace(' ', "%20"));
        assert_eq!(pasted_paths(&url), PastedPaths::Images(vec![first]));
        assert_eq!(
            pasted_paths(&notes.display().to_string()),
            PastedPaths::Unsupported(notes)
        );
        assert_eq!(pasted_paths("2 eggs and toast"), PastedPaths::NotPaths);
        assert_eq!(
            pasted_paths(&root.path().join("missing.png").display().to_string()),
            PastedPaths::NotPaths
        );
        assert_eq!(pasted_paths("  \n"), PastedPaths::NotPaths);
    }
}
