# Contributing to Svarog

Thanks for helping make AI-agent waiting time healthier.

## Development setup

Svarog supports macOS and Linux. Install a stable Rust toolchain, then clone the
repository and run:

```bash
cargo test --locked
scripts/svarog
```

`scripts/bootstrap` checks these prerequisites but does not download or update
the toolchain.

The launcher builds `target/release/svarog` and stores all runtime state under
`./.svarog-dev`; it does not replace the production executable or use
production data, credentials, hooks, or daemon port.

`tmux` is optional and only needed to test `svarog session`.

## Storage-aware development commands

The Python 3 standard-library command `scripts/dev-storage` can report and
manage validation-run storage:

```bash
scripts/dev-storage report
scripts/dev-storage run -- cargo test --locked
scripts/dev-storage run --keep -- cargo clippy --all-targets --locked -- -D warnings
scripts/dev-storage cleanup
scripts/dev-storage cleanup --apply
scripts/dev-storage cleanup --build-artifacts --apply
```

`report` is read-only. `run` executes the command from the repository and uses
the shared Cargo target directory, so it does not copy build caches. It captures
combined output in a private `.svarog-runs/run-*/output.log` and records the
status, command, and elapsed time in `result.json`. The command receives
`SVAROG_RUN_DIR`, pointing to a scratch work directory for that run.

Successful runs are removed by default. Failed runs retain their scratch work,
output, and result for seven days so traces, screenshots, and test-generated
diagnostics remain available. Each `run` invocation prunes expired managed runs,
including abandoned unlocked runs after seven days. Interrupted runs retain
diagnostics. `--keep` preserves the entire run indefinitely, including after
success. Cleanup previews expired managed runs by default; `cleanup --apply`
prunes them manually when validation is idle. There is no background daemon.
Build artifacts are only cleaned by explicitly running
`cleanup --build-artifacts --apply`, which also opts into removing Cargo
package, `flycheck0`, and cross-target release directories older than seven
days. Native `target/debug` and `target/release` warm caches are preserved.

Captured diagnostics can contain command output and arguments; do not pass
secrets directly in command arguments or print them to output.

The runner rejects symlink paths and uses locks to skip active managed runs and
Cargo builds. Its cleanup is limited to managed run data and the opted-in build
artifacts; it preserves `.svarog-dev`, source files, and global caches, and does
not manage arbitrary roots or global installs. Ordinary direct Cargo commands
remain supported; the runner is useful when you want captured logs and run
results.

## Update a development install

The project launcher detects source changes and offers to rebuild before it
continues. Force a rebuild with:

```bash
scripts/svarog --update run
```

Control the prompt with `SVAROG_UPDATE=ask`, `always`, or `never`. Development
builds remain isolated even when a production `svarog` is installed on `PATH`.
The internal `scripts/svarog --build-only` mode rebuilds and records the source
fingerprint without launching Svarog; Settings uses it so a failed build can
return to the previous development binary.

## Before opening a pull request

Run the same checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 -m unittest discover -s tests -p 'test_*.py'
```

Keep changes focused, add tests for behavior changes, and update the README
when a command, platform requirement, privacy behavior, or data flow changes.

When `Cargo.lock` changes, regenerate the bundled license notices before
opening the pull request:

```bash
cargo install cargo-about --version 0.9.1 --locked --features cli
cargo fetch --locked
cargo about generate --frozen about.hbs > THIRD_PARTY_NOTICES.html
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
the release-assets workflow then attaches macOS and Linux archives, their
checksums, and a version-bound shell installer before publishing `svarog-cli`
to crates.io.

## Crates.io publishing

The crates.io package is named `svarog-cli`; its binary remains `svarog`.
Publication uses crates.io trusted publishing, which exchanges the release
workflow's GitHub OIDC identity for a short-lived token. No crates.io token is
stored in the repository.

Configure the publisher once in the `svarog-cli` settings on crates.io:

- Repository owner: `zotttttttt`
- Repository name: `svarog`
- Workflow filename: `release-assets.yml`
- Environment: `publish`

Create the matching `publish` environment in the GitHub repository settings.
Require maintainer approval, disable administrator bypass, and allow deployments
from `main` and `v*` tags. The release workflow verifies that the tag belongs to
a published GitHub Release and exactly matches the Cargo package version, waits
for all release assets to upload, performs a dry run, and then publishes.
Rerunning an already-published version is safe.

To publish an existing release after enabling trusted publishing, manually run
the **Release assets** workflow with its tag, such as `v0.7.3`. This rebuilds
and verifies the release assets before attempting crates.io publication.

The first publication of a new crate name must still be performed manually so
crates.io can establish ownership before trusted publishing is configured:

```bash
cargo publish --dry-run --locked
cargo publish --locked
```

Confirm that both `cargo install svarog-cli --locked` and
`cargo binstall svarog-cli` install a binary whose `svarog --version` matches
the tag.

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
