# Installation

[← Back to the README](../README.md)

## Quick install

The shortest supported path downloads the latest release installer and then
runs it:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  -o /tmp/svarog-installer.sh \
  https://github.com/zotttttttt/svarog/releases/latest/download/svarog-installer.sh
bash /tmp/svarog-installer.sh
$HOME/.local/bin/svarog
```

The installer selects the release for your computer, verifies the binary
against an embedded SHA-256 checksum, and installs `svarog` to
`$HOME/.local/bin`. It does not use `sudo` or edit shell configuration. If the
directory is not already on `PATH`, the installer prints the exact line to add
for future runs.

This path trusts HTTPS and GitHub to deliver the installer. The checksum then
ensures the downloaded binary matches that installer. To authenticate the
installer itself against Svarog's GitHub Actions release build, use the verified
path below.

## Verify release provenance

Install the [GitHub CLI](https://cli.github.com/), then download the versioned
installer and verify its GitHub build-provenance attestation before running it:

```bash
release="$(gh release view --repo zotttttttt/svarog --json tagName --jq .tagName)"
gh release download "$release" --repo zotttttttt/svarog \
  --pattern svarog-installer.sh --clobber
gh attestation verify svarog-installer.sh --repo zotttttttt/svarog
bash svarog-installer.sh
```

Release archives, `SHA256SUMS`, and the installer all carry GitHub
build-provenance attestations. The installer also embeds the expected checksum
for each supported archive, so the downloaded executable is checked again
before installation.

## Upgrade or choose a directory

Use **Svarog version → Check for updates** in Settings to install the latest
release and restart automatically. Interactive production launches also check
periodically and offer each newly published version once. You can still repeat
either installer flow to upgrade manually.

Set `SVAROG_INSTALL_DIR` when running the downloaded installer to choose another
absolute directory:

```bash
SVAROG_INSTALL_DIR="$HOME/bin" bash /tmp/svarog-installer.sh
```

## Install with Cargo

If Rust and
[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) are installed,
download a prebuilt release binary:

```bash
cargo binstall svarog-cli
```

Or compile and install the crate locally:

```bash
cargo install svarog-cli --locked
```

The crates.io package is named `svarog-cli`; both commands install the `svarog`
executable.

## Install a release manually

Download the archive and `SHA256SUMS` from the
[latest release](https://github.com/zotttttttt/svarog/releases/latest):

| Platform | Target |
| --- | --- |
| Apple Silicon macOS | `aarch64-apple-darwin` |
| Intel macOS | `x86_64-apple-darwin` |
| 64-bit Intel/AMD Linux | `x86_64-unknown-linux-gnu` |

Verify the archive's attestation and checksum, extract it, and place the binary
on your `PATH`:

```bash
archive="svarog-VERSION-TARGET"
gh attestation verify "$archive.tar.gz" --repo zotttttttt/svarog
# Linux: grep "  $archive.tar.gz$" SHA256SUMS | sha256sum --check
# macOS: grep "  $archive.tar.gz$" SHA256SUMS | shasum -a 256 --check
tar -xzf "$archive.tar.gz"
mkdir -p "$HOME/.local/bin"
install -m 755 "$archive/svarog" "$HOME/.local/bin/svarog"
```

## Run from a source checkout

Building from source requires a stable Rust toolchain:

```bash
scripts/bootstrap
scripts/svarog
```

The bootstrap checks for Rust but never downloads or installs it. The launcher
builds the current checkout under `target/release` and opens it as a visibly
marked development instance. It never replaces the production `svarog`
executable.

Development data is isolated under `./.svarog-dev`, including its own SQLite
database, configuration, credentials, hooks, and daemon port. The development
launcher cannot use or modify production state.

After changing the checkout, the launcher offers to rebuild before continuing.
Force a rebuild without the prompt with:

```bash
scripts/svarog --update
```

Use `SVAROG_UPDATE=ask`, `always`, or `never` to control rebuild prompting in
scripts and automated workflows.

## Platform notes

- Svarog release binaries support Apple Silicon macOS, Intel macOS, and x86-64
  Linux.
- The macOS binaries are not currently Apple code-signed or notarized.
- `tmux` is optional and only required for `svarog session codex`.
- Linux desktop notifications require a graphical session, a notification
  daemon, and `notify-send` (`libnotify-bin` on Debian and Ubuntu).
