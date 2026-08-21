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
changes. Useful advanced settings include the daily forge ceiling (completed
forges, not repetitions), notification behavior, recommender selection,
timeouts, fallback behavior, and the Codex/OpenAI model configuration.

## Forge archetypes

An archetype gives recommendations a long-term direction without overriding
your current ability, equipment, activity, fatigue, or pain. Change it at any
time without losing workout history.

| Archetype | Long-term direction |
| --- | --- |
| Boxer | Calisthenics, footwork, core work, conditioning, and distributed training volume |
| Wrestler | Pulling, grip, posterior chain, carries, isometrics, and explosive strength |
| Martial Artist | Speed, coordination, balance, core control, and explosive movement |
| Bodybuilder | Resistance, progressive overload, and hypertrophy-focused strength work |
| Runner | Aerobic fitness, movement volume, and durable lower-body endurance |
| Athlete | Balanced strength, muscle, cardio, mobility, coordination, and power |
| Gymnast | Relative strength, core control, balance, mobility, and precise movement |
| Yogi | Mobility, flexibility, balance, breathing, and controlled body awareness |
| Mover | Posture, core work, mobility, balance, and low-impact muscular endurance |
| Thinker | Focus, energy, mood, stress regulation, sleep, and cognitive performance |
| Lifer | Long-term strength, mobility, aerobic fitness, and physical independence |

Choose **Custom** to name your own physical north star. Custom archetypes use
the balanced Athlete behavior while passing your chosen direction to the
recommender.

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
