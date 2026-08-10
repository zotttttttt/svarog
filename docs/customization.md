# Customization

[← Back to the README](../README.md)

## Profile and preferences

Run `svarog setup` to revisit onboarding while preserving workout history and
the current movement. Setup covers measurements, goals, equipment, exercise
preferences, work position, available hands, limitations, intensity, interval,
and notifications.

Advanced settings live in:

```text
~/.config/svarog/config.toml
```

Stop Svarog before editing the file so a running process cannot overwrite your
changes. Useful settings include forge frequency and intensity, daily limits,
notification behavior, recommender selection, timeouts, fallback behavior, and
the Codex/OpenAI model configuration.

## Recommendation prompts

The repository includes two [MiniJinja](https://github.com/mitsuhiko/minijinja)
templates:

- `prompts/exercise_profile.j2` normalizes the user's available equipment.
- `prompts/recommendation_queue.j2` requests future movement candidates.

For personal overrides that survive updates, copy either template to the same
filename under:

```text
~/.config/svarog/prompts/
```

Svarog reloads overrides for each recommendation request, so no rebuild or
restart is necessary. Invalid overrides fall back to conservative local
recommendations when local fallback is enabled.

The queue template receives `context` and `needed`; the profile template
receives `config`. Structured values support `|tojson(indent=2)`.

Keep the output and safety constraints intact: remote results must match
Svarog's schemas, use canonical exercise IDs, respect available equipment, and
remain short enough for coding-agent pauses.
