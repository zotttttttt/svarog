# Data and privacy

[← Back to the README](../README.md)

Svarog keeps its profile, workout state, and local recommendation data on your
machine. Coding prompt text is neither stored nor included in recommendation
requests.

## File locations

| Data | Default location |
| --- | --- |
| Configuration and prompt overrides | `~/.config/svarog/` |
| Collector authentication token | `~/.config/svarog/collector.token` |
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

The dashboard owns a local event collector. It binds only to a loopback address
and requires an owner-only bearer token for event submissions. The token is
stored in the Svarog config directory as `collector.token` with user-only
permissions and rotates whenever the collector starts.
Closing the dashboard stops collection.

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

An OpenAI API key can be read from the configured environment variable or saved
through Settings. Saved keys are protected by macOS Keychain or Linux Secret
Service and are never written to Svarog's config file or database. While the
saved-key recommender is active, Svarog keeps the key in a synchronized,
zeroized process-memory cache to avoid repeated credential-store prompts. It
clears that copy when another recommender is applied, the key is removed, or
Svarog exits. Codex and OpenAI token totals are tracked separately.

## Notifications

Desktop notifications are available on macOS and Linux and are optional during
setup. On Linux, they require a graphical session with a Freedesktop-compatible
notification daemon and `notify-send`; on distributions other than Debian and
Ubuntu, the package providing that command may have a different name.
Notification delivery is best-effort and a missing command or unavailable
desktop service does not affect recommendations. Change the setting at any time
under `preferences.desktop_notifications` in the config file.

## Remove or isolate data

Run `svarog setup --reset` to erase production profile and activity data after
typing the exact `destroy all` confirmation. The reset also attempts to remove
the production saved OpenAI key. If the operating system credential store is
unavailable, Svarog warns and continues resetting its local data. Local database
records are securely deleted, the database is compacted, SQLite write-ahead data
is truncated, and the collector bearer token is rotated. See [Commands](commands.md)
for the full reset behavior.

For testing, `svarog demo` uses only `./.svarog-dev` and a separate development
credential; it does not touch production data, credentials, or hooks.
