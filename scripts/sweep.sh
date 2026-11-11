#!/usr/bin/env bash
# Keeps the Cargo target directory bounded with cargo-sweep
# (https://github.com/holmgr/cargo-sweep). Git hooks run it (lefthook.yml);
# run it by hand after long build loops. Never fails the caller.
#   1. drop artifacts unused for CARGO_SWEEP_DAYS days (default 3)
#   2. drop artifacts built by toolchains that are no longer installed
#   3. trim the oldest artifacts until target/ fits CARGO_SWEEP_MAX_SIZE (default 20GB)
#   4. cap the incremental cache at CARGO_INCREMENTAL_MAX_GB (default 8);
#      cargo-sweep leaves it alone, and dropping it only slows the next build
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || exit 0

if ! command -v cargo-sweep >/dev/null 2>&1; then
  echo "sweep: cargo-sweep is not installed (brew install cargo-sweep, or cargo install cargo-sweep --locked)" >&2
  exit 0
fi
target=${CARGO_TARGET_DIR:-target}
[ -d "$target" ] || exit 0

cargo sweep --time "${CARGO_SWEEP_DAYS:-3}" . 2>&1 | grep -v '^\[INFO\] Would' || true
cargo sweep --installed . 2>&1 | grep -v '^\[INFO\] Would' || true
cargo sweep --maxsize "${CARGO_SWEEP_MAX_SIZE:-20GB}" . 2>&1 | grep -v '^\[INFO\] Would' || true

cap=${CARGO_INCREMENTAL_MAX_GB:-8}
for dir in "$target"/*/incremental; do
  [ -d "$dir" ] || continue
  size=$(du -sg "$dir" | cut -f1)
  [ "$size" -le "$cap" ] && continue
  find "$dir" -mindepth 1 -maxdepth 1 -mtime +0 -exec rm -rf {} +
  size=$(du -sg "$dir" | cut -f1)
  [ "$size" -gt "$cap" ] && rm -rf "$dir"
  echo "sweep: trimmed $dir"
done
exit 0
