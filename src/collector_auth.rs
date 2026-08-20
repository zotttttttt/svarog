use crate::config::Paths;
use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub const TOKEN_BYTES: usize = 32;

pub fn load(paths: &Paths) -> Result<Zeroizing<String>> {
    read(paths)
}

pub fn rotate(paths: &Paths) -> Result<Zeroizing<String>> {
    paths.ensure()?;
    let path = paths.collector_token_file();
    let token = generate()?;
    let mut temporary = tempfile::NamedTempFile::new_in(&paths.config_dir)
        .with_context(|| format!("creating temporary token in {}", paths.config_dir.display()))?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .context("securing temporary collector token")?;
    writeln!(temporary, "{}", token.as_str()).context("writing collector token")?;
    temporary
        .as_file()
        .sync_all()
        .context("syncing collector token")?;
    temporary
        .persist(&path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(token)
}

pub fn bearer_matches(header: Option<&str>, token: &str) -> bool {
    let Some(provided) = header.and_then(|value| value.strip_prefix("Bearer ")) else {
        return false;
    };
    provided.len() == token.len() && bool::from(provided.as_bytes().ct_eq(token.as_bytes()))
}

fn read(paths: &Paths) -> Result<Zeroizing<String>> {
    let path = paths.collector_token_file();
    let metadata =
        fs::symlink_metadata(&path).with_context(|| format!("inspecting {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("collector token must be a regular file: {}", path.display());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("securing {}", path.display()))?;
    let mut contents = String::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_to_string(&mut contents)
        .with_context(|| format!("reading {}", path.display()))?;
    let token = contents.strip_suffix('\n').unwrap_or(&contents);
    if token.len() != TOKEN_BYTES * 2
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("collector token is malformed: {}", path.display());
    }
    Ok(Zeroizing::new(token.to_string()))
}

fn generate() -> Result<Zeroizing<String>> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).context("generating collector token")?;
    let mut token = Zeroizing::new(String::with_capacity(TOKEN_BYTES * 2));
    for byte in bytes {
        use std::fmt::Write as _;
        write!(token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn token_is_user_only_and_rotates() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let first = rotate(&paths).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(load(&paths).unwrap().as_str(), first.as_str());
        assert_eq!(
            fs::metadata(paths.collector_token_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let second = rotate(&paths).unwrap();
        assert_ne!(second.as_str(), first.as_str());
        assert_eq!(load(&paths).unwrap().as_str(), second.as_str());
    }

    #[test]
    fn malformed_and_symlinked_tokens_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("malformed"));
        paths.ensure().unwrap();
        fs::write(paths.collector_token_file(), "short\n").unwrap();
        assert!(load(&paths).is_err());

        let linked = Paths::from_root(root.path().join("linked"));
        linked.ensure().unwrap();
        let target = root.path().join("target");
        fs::write(&target, "a".repeat(64)).unwrap();
        symlink(target, linked.collector_token_file()).unwrap();
        assert!(load(&linked).is_err());
    }

    #[test]
    fn bearer_header_must_match_exactly() {
        let token = "a".repeat(64);
        assert!(bearer_matches(Some(&format!("Bearer {token}")), &token));
        assert!(!bearer_matches(Some(&format!("bearer {token}")), &token));
        assert!(!bearer_matches(Some("Bearer wrong"), &token));
        assert!(!bearer_matches(None, &token));
    }
}
