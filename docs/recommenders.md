# Recommenders

[← Back to the README](../README.md)

Svarog validates every recommendation against your available equipment, safety
profile, movement history, cooldowns, and local exercise catalog. The selected
recommender changes how candidates are generated, not those safeguards.

## Choose a recommender

On the waiting screen, press `s`, focus **Recommender** with `↑`/`↓`, and use
`←`/`→` to cycle through:

| Recommender | Behavior |
| --- | --- |
| Local | Conservative built-in rules with no LLM or network request; the default |
| OpenAI API | Uses the Responses API and records its token usage separately |
| Codex | Uses your installed Codex CLI without requiring a separate API key |

The choice is saved when **Apply changes** is selected. Svarog preserves the
current movement and old safe queue until a newly generated replacement is
ready.

## OpenAI API setup

Make `OPENAI_API_KEY` available to the shell that starts Svarog:

```bash
export OPENAI_API_KEY="..."
svarog
```

Add the export to your shell profile if you want it available in future
sessions. Svarog reads the key from the environment and never writes it to the
config file or database.

The default OpenAI model and reasoning effort can be changed under
`recommender.openai` in the config file.

## Codex setup

Codex recommendations use the installed `codex` command. Setup installs the
Svarog hook and configures a lean, read-only Codex execution for recommendation
generation. The command, arguments, model, and timeout remain configurable
under `recommender.codex` and `recommender`.

## Queues and fallback

Svarog prepares ten future movements at a time. When one remains, it refills
the queue while keeping the current movement available. Remote output is
validated locally; invalid or missing positions are filled with conservative
local recommendations when `local_fallback` is enabled.

Remote failures can be surfaced in the dashboard with `show_llm_failures`.
See [Data and privacy](data-and-privacy.md) for the exact information each
backend can receive.
