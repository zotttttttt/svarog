# Customization

[← Back to the README](../README.md)

## Profile and preferences

Run `svarog setup` to revisit onboarding while preserving workout history and
the current movement. Setup covers measurements, goals, equipment, exercise
preferences, work position, available hands, limitations, notifications, and
Forge archetype. Most profile fields can also be edited from the waiting
dashboard by pressing `s`.

Advanced settings live in:

```text
~/.config/svarog/config.toml
```

Stop Svarog before editing the file so a running process cannot overwrite your
changes. Useful advanced settings include the daily safety ceiling,
notification behavior, recommender selection, timeouts, fallback behavior, and
the Codex/OpenAI model configuration.

## Recommendation prompts

The repository includes two base
[MiniJinja](https://github.com/mitsuhiko/minijinja) templates plus archetype
partials:

- `prompts/exercise_profile.j2` normalizes the user's available equipment.
- `prompts/recommendation_queue.j2` requests future movement candidates.
- `prompts/archetypes/*.j2` supplies the selected long-term training bias.

For personal overrides that survive updates, copy a template to the same
relative path under:

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
remain short enough to complete while your coding agent works.
