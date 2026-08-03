use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub fn fingerprint(root: &Path) -> io::Result<String> {
    let mut hash = FNV_OFFSET_BASIS;

    for path in source_files(root)? {
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        hash_bytes(&mut hash, relative.to_string_lossy().as_bytes());
        hash_bytes(&mut hash, &[0]);
        hash_bytes(&mut hash, &fs::read(path)?);
        hash_bytes(&mut hash, &[0xff]);
    }

    Ok(format!("{hash:016x}"))
}

pub fn source_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for name in ["Cargo.toml", "Cargo.lock", "build.rs"] {
        let path = root.join(name);
        if path.is_file() {
            files.push(path);
        }
    }

    collect_files(&root.join("src"), &mut files)?;
    collect_files(&root.join("prompts"), &mut files)?;
    files.sort_by(|left, right| {
        left.strip_prefix(root)
            .unwrap_or(left)
            .cmp(right.strip_prefix(root).unwrap_or(right))
    });
    Ok(files)
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_changes_with_source() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::create_dir(root.path().join("prompts")).unwrap();
        fs::write(root.path().join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(root.path().join("Cargo.lock"), "# lock\n").unwrap();
        fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.path().join("prompts/queue.j2"), "{{ context }}\n").unwrap();

        let first = fingerprint(root.path()).unwrap();
        assert_eq!(first, fingerprint(root.path()).unwrap());

        fs::write(root.path().join("src/main.rs"), "fn main() { todo!() }\n").unwrap();
        assert_ne!(first, fingerprint(root.path()).unwrap());

        let before_prompt_change = fingerprint(root.path()).unwrap();
        fs::write(root.path().join("prompts/queue.j2"), "{{ context.name }}\n").unwrap();
        assert_ne!(before_prompt_change, fingerprint(root.path()).unwrap());
    }

    #[test]
    fn source_files_are_sorted_relative_to_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::write(root.path().join("Cargo.toml"), "manifest").unwrap();
        fs::write(root.path().join("src/z.rs"), "z").unwrap();
        fs::write(root.path().join("src/nested/a.rs"), "a").unwrap();

        let files = source_files(root.path()).unwrap();
        let relative: Vec<_> = files
            .iter()
            .map(|path| path.strip_prefix(root.path()).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            relative,
            vec![
                PathBuf::from("Cargo.toml"),
                PathBuf::from("src/nested/a.rs"),
                PathBuf::from("src/z.rs"),
            ]
        );
    }
}
