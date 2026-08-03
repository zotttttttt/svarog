#[path = "src/source_fingerprint.rs"]
mod source_fingerprint;

use std::env;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    println!("cargo:rerun-if-changed={}", root.join("src").display());
    println!("cargo:rerun-if-changed={}", root.join("prompts").display());
    let files = source_fingerprint::source_files(&root).expect("enumerate Svarog source files");
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let fingerprint = source_fingerprint::fingerprint(&root).expect("fingerprint Svarog source");
    println!("cargo:rustc-env=SVAROG_SOURCE_ROOT={}", root.display());
    println!("cargo:rustc-env=SVAROG_SOURCE_FINGERPRINT={fingerprint}");
}
