<p align="center">
  <img width="220" src="./assets/svarog-emblem.svg" alt="Svarog logo: an ember-lit smith at an anvil">
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
  <a href="https://github.com/zotttttttt/svarog/releases/latest"><img src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fzotttttttt%2Fsvarog%2Fbadges%2Fbinary-size.json&amp;style=flat-square" alt="Release binary size range"></a>
  &nbsp;
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-888888?style=flat-square&amp;labelColor=070808" alt="Supported platforms: macOS and Linux">
  &nbsp;
  <a href="https://github.com/zotttttttt/svarog/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-888888?style=flat-square&amp;labelColor=070808" alt="MIT license"></a>
</p>

Svarog is a local terminal dashboard that notices when Codex starts working and
turns that wait into a tiny workout.

Svarog adapts each forge to what you complete. It balances sides and recovery,
adjusts reps to your performance, and backs off after fatigue, skips, or pain.

Svarog can’t feel what you feel, so always stop if something doesn’t feel right.

<p align="center">
  <a href="./assets/2-idle.png">
    <img width="620" src="./assets/2-idle.png" alt="Svarog dashboard ready with Forge, Fuel, and API summaries">
  </a>
</p>

## Get started

Install the latest release on macOS or x86-64 Linux. Rust and `sudo` are not
required.

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  -o /tmp/svarog-installer.sh \
  https://github.com/zotttttttt/svarog/releases/latest/download/svarog-installer.sh
bash /tmp/svarog-installer.sh
$HOME/.local/bin/svarog
```

The installer selects the binary for your computer, verifies its embedded
SHA-256 checksum, and installs it to `~/.local/bin`. If that directory is not on
your `PATH`, it prints the exact line to add for future runs.

On first run, press Enter to accept the conservative defaults. Setup connects
Svarog to Codex lifecycle hooks and opens the dashboard. Keep it open while you
work in any Codex terminal. Codex may ask once to trust the hook.

Prefer to open both tools together? With `tmux` installed, run:

```bash
svarog session codex
```

For provenance verification, Cargo, manual downloads, upgrades, and source
builds, see [Installation](docs/installation.md).

## How it works

1. You submit a task to Codex.
2. Codex hooks tell the local Svarog dashboard that work has started.
3. Svarog prepares a safe movement short enough to finish during the wait.
4. Your completions, changed reps, skips, fatigue, and pain shape what comes
   next.

A ready movement remains available until you finish, skip, or report pain—even
if the Codex turn that triggered it has already ended. Coding prompt text is
never collected or sent in recommendation requests.

## Forge while Codex works

Start the suggested movement with `f`, record it with Enter, or adjust the reps
with `+` and `-`. Use `i` for instructions, `s` to skip or report fatigue, and
`p` to report pain and block the exercise.

<p align="center">
  <a href="./assets/3-forging.png">
    <img width="44%" src="./assets/3-forging.png" alt="Svarog movement session showing a one-arm kettlebell row target and session controls">
  </a>
  &nbsp;
  <a href="./assets/4-how-to.png">
    <img width="44%" src="./assets/4-how-to.png" alt="Svarog terminal instructions for performing a one-arm kettlebell row">
  </a>
</p>
<p align="center"><sub>Complete a movement or open step-by-step instructions without leaving the dashboard.</sub></p>

When an exercise has reference images, press `o` from its instructions to open
a local visual guide in your browser.

<p align="center">
  <a href="./assets/5-how-to-images-in-browser.png">
    <img width="720" src="./assets/5-how-to-images-in-browser.png" alt="Local Svarog visual guide with exercise instructions and two reference positions">
  </a>
</p>

> Svarog starts conservatively, respects reported pain and fatigue, and excludes
> exercises that require a partner. Stop any movement that hurts. Svarog is not
> medical advice.

## Make it yours

Choose a Forge archetype—such as Boxer, Yogi, Thinker, or Lifer—to give your
training a long-term direction. Your actual performance and safety feedback
still determine today's pace. You can change the archetype, profile, equipment,
and preferences at any time without losing history.

<p align="center">
  <a href="./assets/0-choose-your-fighter.png">
    <img width="52%" src="./assets/0-choose-your-fighter.png" alt="Svarog Forge archetype selector showing the Thinker archetype and capability stats">
  </a>
  &nbsp;
  <a href="./assets/7-settings.png">
    <img width="42%" src="./assets/7-settings.png" alt="Svarog Settings showing archetype, recommender, profile fields, and controls">
  </a>
</p>
<p align="center"><sub>Pick a physical north star during setup, then refine your profile from Settings.</sub></p>

Svarog works fully offline with its conservative Local recommender. You can
optionally use your installed Codex CLI or an OpenAI API key to generate future
movements. Every result is validated locally against your equipment, safety
profile, cooldowns, and exercise catalog. See [Recommenders](docs/recommenders.md)
for setup and fallback behavior.

## See your progress

Open Forge, Fuel, or API from the dashboard to review recent workouts, nutrition
and water, or recommender usage. Press `a` to log food or adjust today's water;
meal parsing requires the Codex or OpenAI recommender, while water stays local.

<p align="center">
  <a href="./assets/8-stats-forges.png">
    <img width="30%" src="./assets/8-stats-forges.png" alt="Svarog Forge statistics showing today's and this week's completed movements and reps">
  </a>
  &nbsp;
  <a href="./assets/9-stats-fuel.png">
    <img width="30%" src="./assets/9-stats-fuel.png" alt="Svarog Fuel statistics showing calories, macros, and weight trend">
  </a>
  &nbsp;
  <a href="./assets/10-stats-api.png">
    <img width="30%" src="./assets/10-stats-api.png" alt="Svarog API statistics showing token counts and cost">
  </a>
</p>
<p align="center"><sub>Workouts, fuel, hydration, weight trend, and recommender usage stay easy to inspect.</sub></p>

## Everyday controls

| Key | Action |
| --- | --- |
| `↑` / `↓`, Enter, Esc | Navigate dashboard views |
| `f` | Start the next movement now |
| `a` | Add fuel or update water |
| `s` | Open Settings, including app updates; skip or report fatigue during a movement |
| `i` / `?` | Read movement instructions |
| `d` / Enter | Finish and record the displayed reps |
| `p` | Report pain and block the current exercise |

See [Commands](docs/commands.md) for all dashboard controls, CLI commands,
reset behavior, and isolated demo mode.

## Documentation

| Guide | What it covers |
| --- | --- |
| [Installation](docs/installation.md) | Verified installs, Cargo, upgrades, source builds, and platform notes |
| [Commands](docs/commands.md) | Dashboard controls, CLI commands, demo mode, and reset |
| [Customization](docs/customization.md) | Forge archetypes, profile settings, config, and prompt overrides |
| [Recommenders](docs/recommenders.md) | Local, Codex, OpenAI, API keys, queues, and fallback |
| [Fuel and water](docs/fuel-and-water.md) | Meal logging, nutrition estimates, dates, and hydration |
| [Data and privacy](docs/data-and-privacy.md) | Stored data, network boundaries, credentials, and deletion |

## Project

Svarog uses a compact local copy of
[free-exercise-db](https://github.com/yuhonas/free-exercise-db) at
[revision `b0eed061`](https://github.com/yuhonas/free-exercise-db/commit/b0eed061e1c832b3ed815fbaa4b45b3cdc14df49),
published under the [Unlicense](https://github.com/yuhonas/free-exercise-db/blob/main/LICENSE.md).
Instructions stay local; reference images download only when requested and are
then cached locally.

Svarog is built with Rust and Ratatui and released under the [MIT License](LICENSE).
Contributions are welcome—see [CONTRIBUTING.md](CONTRIBUTING.md). The complete
dependency and license inventory is in [THIRD_PARTY_NOTICES.html](THIRD_PARTY_NOTICES.html).
