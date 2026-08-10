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

When `Cargo.lock` changes, regenerate the bundled license notices before
opening the pull request:

```bash
cargo install cargo-about --version 0.9.1 --locked --features cli
cargo about generate --locked about.hbs > THIRD_PARTY_NOTICES.html
```

## Release labels

Every pull request gets one release label. New pull requests default to
`semver:patch`; select a different label when appropriate:

| Label | Result |
| --- | --- |
| `semver:breaking` | Breaking change; minor before 1.0, major afterward |
| `semver:minor` | Backward-compatible feature |
| `semver:patch` | Backward-compatible fix or improvement |
| `semver:none` | No published release, such as internal release maintenance |

The release metadata workflow keeps these labels mutually exclusive and adds
the matching conventional prefix to the pull request title. Pull requests must
be squash-merged so that Release Please can use that title as the release
commit. As changes reach `main`, Release Please maintains a release PR with the
next version and changelog. Merging that PR creates the tag and GitHub Release;
the release-assets workflow then attaches macOS and Linux archives and their
checksums.

## One-time release automation setup

Repository maintainers must install a dedicated GitHub App with read access to
metadata and write access to contents, issues, and pull requests. Add its
credentials as Actions secrets named `RELEASE_APP_ID` and
`RELEASE_APP_PRIVATE_KEY`. Using the App identity ensures its release PRs
trigger the normal pull request checks.

Merge the initial automation pull request with the squash title
`chore: set up automated releases`; its metadata workflow is not active until
after that merge. Then enable squash merging for `main` and require the Rust CI
check and the `release-metadata` commit status. Publish `v0.2.0` at the setup
merge commit as the baseline GitHub Release. Publishing the baseline also
builds its downloadable assets. Later releases are fully automated by merging
the Release Please PR.

## Safety and privacy

Never commit workout databases, configuration files, API keys, prompt content,
logs, or other user data. Exercise recommendations must remain conservative,
respect reported injuries and pain, and retain a local non-LLM fallback.
