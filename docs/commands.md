# Commands

[← Back to the README](../README.md)

## Everyday use

| Command | Purpose |
| --- | --- |
| `svarog` / `svarog run` | Set up if necessary, then open the dashboard and collector |
| `svarog session codex` | Open Codex and Svarog together in a tmux session |
| `svarog status` | Print the current state and recommendation |
| `svarog stop` | Stop Svarog runtimes and Svarog-created tmux sessions |
| `svarog setup` | Repair or revisit setup, then open the dashboard |
| `svarog --help` | Show the current command reference |

Closing the dashboard stops its local event collector. `svarog stop` also
closes coding-agent processes inside tmux sessions created by Svarog.

While the dashboard is waiting, press `s` to edit the Forge archetype,
recommender, notifications, daily forge ceiling, measurements, goals,
equipment, work setup, limitations, exercise preferences, and a securely saved
OpenAI API key. Profile and recommender changes remain staged until you press
Ctrl+S (or Command+S when supported by the terminal); Esc cancels those
changes. Changing the recommender refreshes future forges; other Settings saves
keep the compatible queue. Height, weight, age, and choice fields can be
adjusted with Left/Right, while Enter opens selectors and exact-value editors.
API keys are masked and saved immediately in the operating system credential
store, not `config.toml`.
Replacing or removing a key is also immediate and independent of applying the
selected recommender.

Applied weight changes are retained as local check-ins. Once you have changed a
saved weight, the dashboard shows total weight lost or gained since the first
recorded check-in in your selected unit system.

After `svarog setup` completes, press Enter at the final prompt to open the
dashboard immediately. `svarog setup --dry-run` prints its preview and exits.

## Exercise controls

| Command | Purpose |
| --- | --- |
| `svarog start` | Mark the current recommendation active |
| `svarog done` | Complete the current movement |
| `svarog skip` | Skip the current movement |
| `svarog pain` | Report pain and block the current exercise |
| `svarog exercises removed` | List exercises removed from recommendations |
| `svarog exercises restore ID` | Restore one removed exercise |
| `svarog exercises restore-all` | Restore every user-removed exercise |

Partner-required exercises are excluded by Svarog itself and cannot be restored.

## Reset

To erase your profile, activity history, and current state, stop the dashboard
and run:

```bash
svarog setup --reset
```

You must type `destroy all` before anything is removed. The installed binary
and Codex integration files remain in place. Svarog also removes its saved
OpenAI API key from the operating system credential store. If that store is
unavailable, the data reset still completes and prints a warning explaining
that the credential may need to be removed manually.

## Demo and safe setup checks

`svarog demo` opens an isolated environment under `./.svarog-dev`. It does not
touch production data, credentials, or hooks. Removing existing demo data also
removes the development-scoped saved OpenAI key when the credential store is
available.

Use dry-run and development modes to inspect setup safely:

```bash
svarog setup --dry-run
svarog setup --dev
svarog setup --dev --dry-run
```

To connect a Codex process explicitly to the development sandbox:

```bash
SVAROG_HOME="$PWD/.svarog-dev/svarog" \
CODEX_HOME="$PWD/.svarog-dev/codex" \
SVAROG_DAEMON_ADDR="127.0.0.1:18787" \
SVAROG_MODE=dev \
codex
```
