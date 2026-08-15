<p align="center">
  <img width="280" src="./svarog.png" alt="Svarog logo: an ember-lit smith at an anvil">
</p>

<h1 align="center">Svarog</h1>

<p align="center">
  Turn AI-agent waiting time into short, adaptive exercise sessions.
</p>

<p align="center">
  <a href="https://github.com/zotttttttt/svarog/releases"><img src="https://img.shields.io/github/v/release/zotttttttt/svarog?label=version&amp;style=flat-square&amp;labelColor=070808&amp;color=FF8C00" alt="Latest Svarog release version"></a>
  &nbsp;
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-FF8C00?style=flat-square&amp;labelColor=070808" alt="Built with Rust"></a>
  &nbsp;
  <a href="https://github.com/zotttttttt/svarog/actions/workflows/ci.yml?query=branch%3Amain"><img src="https://github.com/zotttttttt/svarog/actions/workflows/ci.yml/badge.svg?branch=main&amp;style=flat-square&amp;label=build" alt="Build status"></a>
  &nbsp;
  <a href="https://github.com/zotttttttt/svarog/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-888888?style=flat-square&amp;labelColor=070808" alt="MIT license"></a>
  &nbsp;
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-888888?style=flat-square&amp;labelColor=070808" alt="Supported platforms: macOS and Linux">
  &nbsp;
  <img src="https://img.shields.io/badge/desktop%20notifications-macOS-888888?style=flat-square&amp;labelColor=070808" alt="Desktop notifications available on macOS">
</p>

Svarog is a local Rust CLI and terminal dashboard that turns the time your
coding agent spends executing a task into tiny workouts. It learns from your
equipment, preferences, recent activity, fatigue, and pain—then keeps each
movement short enough to finish while the agent works.

Choose a Forge archetype—the kind of physical capability you want to develop:
Boxer, Wrestler, Athlete, Gymnast, Yogi, Lifer, or another built-in or custom
north star. It biases what Svarog selects over time, never what it assumes you
can do today. Actual completions, changed reps, skips, and pain determine pace.

The loop is simple:

1. You give your coding agent a spec or prompt to execute.
2. As the agent starts working, Svarog prepares a safe, short movement.
3. You finish, skip, or report pain while the agent works.
4. Your history shapes what comes next.

Use the conservative built-in recommender, your installed Codex CLI, or the
OpenAI API. Prompt text is never collected or sent in recommendation requests.

## Get started

You need macOS or Linux. A Rust toolchain is only required when installing from
source. `tmux` is optional and only needed for `svarog session codex`.

### Install a release

Download the archive for your computer from the
[latest release](https://github.com/zotttttttt/svarog/releases/latest):

| Platform | Target |
| --- | --- |
| Apple Silicon macOS | `aarch64-apple-darwin` |
| Intel macOS | `x86_64-apple-darwin` |
| 64-bit Intel/AMD Linux | `x86_64-unknown-linux-gnu` |

Verify it against `SHA256SUMS`, extract it, and place `svarog` on your `PATH`:

```bash
archive="svarog-VERSION-TARGET"
tar -xzf "$archive.tar.gz"
mkdir -p "$HOME/.local/bin"
install -m 755 "$archive/svarog" "$HOME/.local/bin/svarog"
```

Release binaries are not currently code-signed or notarized.

### Install from a checkout

```bash
scripts/bootstrap
scripts/svarog
```

The bootstrap checks Rust; the launcher installs Svarog, guides you through
setup, connects Codex, and opens the dashboard. Press Enter to accept the
conservative defaults.

After setup, run Svarog in its own terminal:

```bash
svarog
```

Or open Svarog and Codex together in a tmux session:

```bash
svarog session codex
```

## Use Svarog

Keep one dashboard open while you work in any of your Codex terminals. A ready
movement stays available until you finish, skip, or report pain—even if the
agent turn that triggered it has ended.

<p align="center">
  <a href="./assets/1-tui-idle.png">
    <img width="420" src="./assets/1-tui-idle.png" alt="Svarog waiting dashboard showing the current recommender, completed exercises, reps, and token usage">
  </a>
</p>
<p align="center"><sub>See what is next, track your progress, or open Settings to update your profile and Forge archetype.</sub></p>

While waiting:

| Key | Action |
| --- | --- |
| `f` | Start the next safe movement now |
| `l` / `n` | View recent / upcoming movements |
| `r` | Regenerate from the upcoming-movements view |
| `s` | Open focus-driven Settings; use ↑/↓ to focus and ←/→ to change a field |

During a movement:

<p align="center">
  <a href="./assets/2-tui-forging.png">
    <img width="44%" src="./assets/2-tui-forging.png" alt="Svarog movement session showing a kettlebell sumo high pull target and session controls">
  </a>
  &nbsp;
  <a href="./assets/3-tui-how-to.png">
    <img width="44%" src="./assets/3-tui-how-to.png" alt="Svarog terminal instructions for performing a kettlebell sumo high pull">
  </a>
</p>
<p align="center"><sub>Record the result or open step-by-step instructions without leaving the dashboard.</sub></p>

| Key | Action |
| --- | --- |
| `d` or Enter | Finish and record the displayed reps |
| `+` / `-` | Adjust the reps you completed |
| `i` or `?` | Read step-by-step instructions |
| `o` | Open the visual guide with reference images from the instructions screen |
| `s` | Skip, report fatigue, or remove the exercise |
| `p` | Report pain and block the exercise |

<p align="center">
  <a href="./assets/4-local-html-how-to.png">
    <img width="760" src="./assets/4-local-html-how-to.png" alt="Local Svarog visual guide with kettlebell sumo high pull instructions and two reference positions">
  </a>
</p>
<p align="center"><sub>When available, reference images open on demand in a local visual guide.</sub></p>

> Svarog starts conservatively, respects reported pain and fatigue, and excludes
> exercises that require a partner. Recommendations are not medical advice;
> stop whenever a movement hurts or feels unsafe.

## Learn more

| Guide | What it covers |
| --- | --- |
| [Recommenders](docs/recommenders.md) | Local, Codex, OpenAI, API-key setup, queues, and fallback |
| [Commands](docs/commands.md) | Setup, daily use, exercise controls, demo, and reset |
| [Data and privacy](docs/data-and-privacy.md) | Stored data, file locations, network boundaries, and deletion |
| [Customization](docs/customization.md) | Profile settings, config, models, and prompt overrides |

## Exercise data

Svarog uses a compact local copy of
[free-exercise-db](https://github.com/yuhonas/free-exercise-db) at
[revision `b0eed061`](https://github.com/yuhonas/free-exercise-db/commit/b0eed061e1c832b3ed815fbaa4b45b3cdc14df49),
published under the [Unlicense](https://github.com/yuhonas/free-exercise-db/blob/main/LICENSE.md).
Partner-required exercises are removed from the canonical pool. Instructions
stay local; reference images download only when you ask for them and are then
cached locally.

Remote recommenders receive compact exercise metadata—not instructions or
images—and Svarog validates every result against the local catalog.

## Built with

[Rust](https://www.rust-lang.org/) · [Ratatui](https://ratatui.rs/) ·
[Crossterm](https://github.com/crossterm-rs/crossterm) ·
[Tokio](https://tokio.rs/) · [Axum](https://github.com/tokio-rs/axum) ·
[SQLite](https://sqlite.org/) · [Rusqlite](https://github.com/rusqlite/rusqlite) ·
[MiniJinja](https://github.com/mitsuhiko/minijinja)

See the [third-party notices](THIRD_PARTY_NOTICES.html) for the complete
dependency and license inventory.

## License and contribute

Svarog is available under the [MIT License](LICENSE). Contributions are
welcome—see [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and
required checks.
