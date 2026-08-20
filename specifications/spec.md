# Svarog — Tool Spec v0.1

## Product

**Svarog** is a Rust CLI/TUI sidecar for AI coding agents.

> While Claude/Codex/Droid works, Svarog forges you.

Primary UX:

```bash
svarog setup
svarog run
codex # in any terminal
```

One TUI collects global Codex lifecycle events. The optional
`svarog session codex` command creates an 80/20 tmux split:

```text
┌──────────────────────────────┬──────────────┐
│ Claude Code / Codex / Droid  │ ✦ Svarog     │
│                              │              │
│ agent work here              │ 10 curls     │
│                              │ [s] start    │
│                              │ [k] skip     │
└──────────────────────────────┴──────────────┘
```

---

## Core Requirements

### 1. Rust-first

Use Rust for:

* CLI
* TUI
* daemon
* SQLite storage
* tmux orchestration
* hook/event server

Suggested crates:

```toml
clap = "4"
ratatui = "0.29"
crossterm = "0.28"
tokio = "1"
axum = "0.7"
rusqlite = "0.32"
serde = "1"
serde_json = "1"
toml = "0.8"
chrono = "0.4"
directories = "5"
libc = "0.2"
```

---

## Commands

```bash
svarog setup
svarog session claude
svarog session codex
svarog session droid
svarog run
svarog status
svarog start
svarog done
svarog skip
svarog pain
```

---

## Modes

### `svarog session <agent>`

Creates tmux layout.

Responsibilities:

* start tmux session
* left pane: selected agent
* right pane: `svarog run`
* install/use hook integration if available

Example:

```bash
svarog session claude
```

---

### `svarog setup`

Single-command first run.

Responsibilities:

* create profile
* collect natural-language exercise preferences
* generate initial exercise profile
* create config and database
* install Codex integration
* tell the user to run `svarog run` while collecting agent work
* ask whether desktop notifications should be enabled

The user should not need to know about config files, calibration, daemons,
hooks, or events during onboarding.

Environment modes:

```bash
svarog setup              # production
svarog setup --dev        # sandbox under ./.svarog-dev
svarog setup --dry-run    # no prompts, no writes
svarog setup --dev --dry-run
```

Defaults:

```text
production svarog: ~/.config/svarog and ~/.local/share/svarog
production codex:  ~/.codex
production daemon: 127.0.0.1:8787
dev svarog:        ./.svarog-dev/svarog
dev codex:         ./.svarog-dev/codex
dev daemon:        127.0.0.1:18787
```

Overrides:

```text
SVAROG_HOME
CODEX_HOME
SVAROG_DAEMON_ADDR
SVAROG_MODE
```

Normal `codex` reads production hooks. To test the dev sandbox hook inside
Codex, setup should print this pattern after `svarog setup --dev`:

```bash
SVAROG_HOME="$PWD/.svarog-dev/svarog" \
CODEX_HOME="$PWD/.svarog-dev/codex" \
SVAROG_DAEMON_ADDR="127.0.0.1:18787" \
SVAROG_MODE=dev \
codex
```

---

### `svarog run`

Single-instance forge panel and owner of the local event collector. Multiple
ordinary Codex terminals report to this collector through the global hook.
Closing the TUI stops Codex collection.

States:

```text
Idle
Opportunity
Active
Cooldown
```

---

### `svarog stop`

Stops production, the current project's demo runtime, and all `svarog-*` tmux
sessions. Stopping a Svarog-created tmux session also terminates the coding
agent inside it.

---

### Internal Event Receiver

Local event receiver.

Example endpoint:

```http
POST localhost:8787/events
```

Event:

```json
{
  "agent": "claude",
  "event": "tool_start",
  "expected_duration_sec": 120,
  "project": "svarog"
}
```

---

## TUI States

### Idle

```text
✦ Svarog

Waiting for the next forge.
[f] Forge now  [a] Add fuel
[l] Latest forges  [n] Next forges
```

Add fuel is also available during cooldown. It reviews Luna-estimated meals and
drinks before saving, keeps plain-water tracking local, and displays today's
water total in the profile's selected unit system. The waiting dashboard shows
today's fuel plus a daily average over the most recent seven distinct local
calendar dates with logged fuel; before seven dates exist, it averages and labels
the available logged-day count.

---

### Opportunity

```text
✦ Svarog

Claude is working.

10 left-arm curls
12 kg kettlebell

[s] Start
[d] Done
[s] Skip
[+/-] Actual reps
```

---

### Active

```text
✦ Svarog

Set active

10 left-arm curls

[d] Done
[k] Skipped
[p] Pain
```

---

### Cooldown

```text
Forged.

Avoid biceps for 18 min.
Next likely: shoulders or walk.
```

---

## UX Rules

* Svarog never steals the main workflow.
* Agent pane remains primary.
* Svarog only uses the right 20%.
* When Codex is the recommender, idle and cooldown views show compact local-day
  and local-week input/output token totals for Svarog's own Codex calls.
* Idle and cooldown views show local-day and local-week completed forge and
  actual-rep totals. Skipped, pain, and started sets do not count.
* From idle and cooldown, `f` opens the 10 latest recorded forge outcomes,
  grouped by local date. `Esc` returns to the waiting view.
* From idle and cooldown, `n` previews the queued recommendations in the order
  Svarog currently plans to promote them. Viewing the queue does not consume it.
  Cooldown is side-aware for unilateral work: completing one side does not block
  an adjacent queued recommendation for the opposite side. Bilateral and legacy
  unsided work continues to cool down the whole muscle group.
  In this preview, `r` regenerates the full queue in the background. The old
  queue stays available until a non-empty replacement is ready; failures leave
  it unchanged. The `r` control becomes an animated loading indicator and is
  restored with a completion checkmark after success. Manual regeneration and
  automatic refill never run concurrently; `r` is a silent no-op while an
  automatic refresh is active.
* When enabled, a desktop notification appears when a forge becomes actionable.
  Each later Codex prompt repeats the same notification while that forge remains
  incomplete. Agent/TUI startup, completion, and queue prefill do not notify.
* Single-key actions inside TUI:

  * `d` done
  * `s` skip
  * `+` / `-` adjust actual reps
  * `y` fatigued skip
  * `n` normal skip
  * `p` pain
  * `f` latest forge history while waiting
  * `n` queued next forges while waiting
  * `r` regenerate the queue from the next-forges preview
  * `q` quit
  * `Esc` cancel the current prompt
* All actions must also work from CLI:

```bash
svarog done
svarog skip
svarog pain
```

---

## Onboarding

### `svarog setup`

Collect:

* measurement unit system (`metric` or `imperial`)
* height
* weight
* age
* goals
* equipment and available weights as natural-language text
* sitting/standing setup
* one-hand/two-hand availability
* cautious body parts
* injuries or hard limitations
* Forge archetype
* daily forge ceiling (completed forges, not repetitions)
* exercise preferences as natural-language text

Store in:

```text
~/.config/svarog/config.toml
```

Metric setup collects height in centimeters and weight in kilograms. Imperial
setup collects height in one field using forms such as `5'11`, `6 ft 1 in`, or
`71 in`, and collects weight in pounds. Measurements are always normalized to
`height_cm` and `weight_kg` in storage; the selected unit system only controls
setup input and displayed defaults. Existing configurations without a
unit-system preference default to metric.

Equipment and exercise preferences are stored as the user's natural-language
text. Equipment eligibility is conservative: bodyweight means no external prop,
fixed equipment such as a pull-up bar, bench, rack, wall, or stable support must
be named explicitly, and equipment quantities are respected. Leaving exercise
preferences blank stores `automatic`.

---

## Calibration

Calibration is removed from normal onboarding.

Initial exercise selection should collect intent, not validate capability.
The LLM generates the starting movement pool from profile, equipment, goals,
injuries, cautious body parts, Forge archetype, demonstrated capacity, and exercise
preferences. Future adaptation happens continuously from user behavior.

---

## Recommendation Engine v1

LLM-backed by default, with a deterministic local heuristic fallback.

Inputs:

* user profile
* equipment as natural-language text
* movement whitelist
* last muscle groups trained
* today’s volume
* today’s agent sessions/events
* user cadence
* expected downtime
* pain blacklist
* current Svarog state

Rules:

```text
Never train to failure.
Never repeat same muscle group twice in a row.
Every 4th intervention: circulation/mobility.
If pain: block movement.
If skipped 3x: reduce difficulty.
If easy 5x: increase reps slightly.
If fatigue skip: suppress next 5 forge opportunities.
```

Backends:

```toml
[recommender]
backend = "codex" # codex, openai_env, openai_keyring, local
timeout_ms = 60000
local_fallback = true
show_llm_failures = true

[recommender.codex]
command = "codex"
args = ["exec", "--skip-git-repo-check", "--sandbox", "read-only"]
model = "gpt-5.6-luna" # "inherit" uses the user's Codex model

[recommender.openai]
api_key_env = "OPENAI_API_KEY"
model = "gpt-5.4-nano"
reasoning_effort = "low"
```

Codex recommendation calls use one timeout budget. An early failure of the
configured model retries once with the user's inherited Codex model; timeout or
completed-turn failures go directly to the configured local fallback.

The OpenAI backends must use the Responses API with structured JSON output, not legacy text generation. `openai_env` reads `api_key_env`; `openai_keyring` reads a key saved through Svarog Settings and protected by macOS Keychain or Linux Secret Service. Saved-key reads are lazy and synchronized so concurrent work causes at most one credential-store authorization. A zeroized in-memory copy may be reused while that backend remains applied, and must be cleared after applying another backend, removing the key, or ending the process. Keys must never be serialized to Svarog configuration or workout storage. The legacy `openai` backend value migrates to `openai_env`. The recommender prompt returns one strict JSON object with either `recommend` or `no_recommendation`. Rust validation remains authoritative for fit, cooldown, repetition, injury conflicts, and max set size.

Recommender prompts are MiniJinja files. Bundled defaults live under `prompts/`;
matching files under `~/.config/svarog/prompts/` override them and are reloaded
for every generation. Templates receive structured `config`, `context`, and
`needed` values and render JSON with `tojson`. Undefined values and invalid
syntax fail strictly and use the conservative local fallback.

---

## Data Model

SQLite:

```sql
users
movements
sessions
events
recommendations
sets
pain_events
```

Set record:

```json
{
  "movement": "left_bicep_curl",
  "weight_kg": 12,
  "reps": 10,
  "status": "done",
  "muscles": ["biceps", "grip"],
  "agent": "claude",
  "project": "svarog"
}
```

---

## Hook Integration

### Claude Code

Use lifecycle hooks to call:

```bash
svarog event --agent claude --event tool_start
```

### Codex

Installed by `svarog setup` as global user-level `SessionStart`,
`UserPromptSubmit`, `Stop`, and `SessionEnd` hooks. A unique submitted turn is a
forge opportunity while the TUI collector is running. Stop/end events update
lifecycle bookkeeping but never clear an open forge.

```bash
svarog codex-hook # hidden hook ingestion command; reads Codex JSON on stdin
```

### Generic

Support manual event:

```bash
svarog event --agent custom --event busy --duration 120
```

Installed hook scripts are executable Svarog-owned scripts under `~/.config/svarog/hooks/`.

---

## Visual Identity in TUI

Theme:

```text
background: near black
text: soft white / gray
primary: ember copper
sparks: rare orange pixels
```

Tone:

```text
dark forge
quiet
minimal
not fitness-app
not gamified
```

---

## MVP Build Order

Initial implementation target:

1. Bootstrap prerequisite check for Rust and tmux
2. `svarog setup`
3. SQLite storage
4. Automatic exercise profile generation
5. LLM-backed recommendation engine with conservative fallback
6. Internal event receiver
7. Generic event API
8. `svarog run`
9. `svarog stop`
10. `svarog session codex`
11. CLI actions: `start`, `done`, `skip`, `pain`
12. Stats/status
13. Hidden hook commands for Codex, Claude, Factory Droid, OpenClaw, and generic API
14. macOS and Linux desktop notifications
15. Vendor-specific hook script installers

The engine treats every submitted agent turn as a movement opportunity. A
local adaptive gate begins around every second eligible opportunity, speeds up
after demonstrated capacity, and backs off after reduced reps, skips, fatigue,
or pain. The Forge archetype biases long-term selection, not immediate ability.

---

## First public README line

```text
Svarog is a CLI sidecar that turns AI-agent waiting time into tiny, adaptive workouts.
```

Better hero line:

```text
Your strength, forged in the background.
```
