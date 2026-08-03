# Contributing to Svarog

Thanks for helping make AI-agent waiting time healthier.

## Development setup

Svarog supports macOS and Linux. Install a stable Rust toolchain and `tmux`,
then clone the repository and run:

```bash
cargo test --locked
cargo run -- demo
```

The demo stores all state under `./.svarog-dev`; it does not use production
Svarog data or hooks.

## Before opening a pull request

Run the same checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Keep changes focused, add tests for behavior changes, and update the README
when a command, platform requirement, privacy behavior, or data flow changes.

## Safety and privacy

Never commit workout databases, configuration files, API keys, prompt content,
logs, or other user data. Exercise recommendations must remain conservative,
respect reported injuries and pain, and retain a local non-LLM fallback.
