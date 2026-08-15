# Data and privacy

[← Back to the README](../README.md)

Svarog keeps its profile, workout state, and local recommendation data on your
machine. Coding prompt text is neither stored nor included in recommendation
requests.

## File locations

| Data | Default location |
| --- | --- |
| Configuration and prompt overrides | `~/.config/svarog/` |
| Workout database | `~/.local/share/svarog/svarog.sqlite3` |
| Codex hook configuration | `~/.codex/hooks.json` |

Svarog creates its config and data directories with user-only permissions on
macOS and Linux. The config and database files are also restricted to the
current user.

The database contains your profile, available movement pool, queue, completed
and skipped movements, reps, pain and fatigue reports, cooldown state, and
recommender token totals. Setup answers can include age, height, weight, goals,
equipment, preferences, cautious body parts, and injuries.

## Local collector

The dashboard owns a local event collector. It binds only to a loopback address;
Svarog rejects non-loopback `SVAROG_DAEMON_ADDR` values because the event API is
not authenticated. Closing the dashboard stops collection.

Codex lifecycle events tell Svarog when you submit a task for Codex to execute.
They do not forward the text of your coding prompts.

## Recommender data flow

| Backend | Data leaves your machine? |
| --- | --- |
| Local | No |
| Codex | A bounded exercise profile or workout context is passed to your installed Codex CLI |
| OpenAI API | The same bounded context is sent to the OpenAI Responses API |

Recommendation context can include the Forge archetype, exercise goals,
equipment, injuries, prescribed-versus-performed outcomes, daily totals, and
cooldown state. Exercise instructions and reference images stay local. Remote
recommenders receive only compact catalog metadata, and every response is
validated locally.

The OpenAI API key is read from the configured environment variable and is not
stored by Svarog. Codex and OpenAI token totals are tracked separately.

## Notifications

Desktop notifications are available on macOS and are optional during setup.
Other Svarog functionality works on both macOS and Linux. Change the setting at
any time under `preferences.desktop_notifications` in the config file.

## Remove or isolate data

Run `svarog setup --reset` to erase production profile and activity data after
typing the exact `destroy all` confirmation. See [Commands](commands.md) for the
full reset behavior.

For testing, `svarog demo` uses only `./.svarog-dev` and does not touch
production data or hooks.
