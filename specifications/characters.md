# Svarog Forge Archetypes — Onboarding & Recommender Specification

## 1. Goal

Replace the current onboarding parameters:

```text
Forge intensity (1-5) [1]:
Forge interval (how many agent runs before prompting you to move) [2]:
```

with a **Forge Archetype**.

The archetype is the user's long-term physical north star. It describes the kind of physical capability Svarog should gradually bias the user toward.

The user does **not** choose:

* how hard today's exercise should be;
* how many repetitions they should perform;
* how frequently they should exercise;
* how much daily volume they should accumulate.

Svarog learns those values dynamically from:

* user profile;
* goals;
* equipment;
* cautious body parts;
* injuries / hard limitations;
* selected archetype;
* previous exercises;
* accumulated daily load;
* skipped exercises;
* user-modified repetition counts;
* explicit feedback where available.

The archetype influences **exercise selection and long-term direction**, not immediate workload.

---

# 2. Position in onboarding

Existing onboarding remains largely unchanged:

```text
Use metric units? (n = imperial) [Y/n]:
Height cm []:
Weight kg []:
Age []:
Goals [consistent movement]:
What equipment do you have near your desk?
[bodyweight only]:
Do you usually sit while agents run?
(n = standing) [Y/n]:
Alternate arms between forging sessions? [Y/n]:
Cautious body parts []:
Injuries or hard limitations []:
```

After these questions, launch the Forge Archetype selector.

Remove:

```text
Forge intensity (1-5) [1]:
Forge interval (how many agent runs before prompting you to move) [2]:
```

from onboarding.

---

# 3. Archetype selector UX

The archetype selector should visually transition from the ordinary line-by-line onboarding into the **normal Svarog runtime UI**.

It should:

* occupy approximately the same terminal area as an active Svarog forge;
* use normal Svarog branding;
* use the same borders / typography / spacing conventions as the runtime interface;
* redraw in place rather than printing every archetype sequentially;
* feel like the user's first interaction with Svarog itself rather than another configuration question.

Example:

```text
╭─────────────────────── SVAROG ───────────────────────╮
│                                                       │
│                      WRESTLER                         │
│                                                       │
│  Powerful pulling, grip, posterior-chain strength,   │
│  carries, isometrics, explosiveness and conditioning.│
│  ← previous   → next   Enter choose   / custom       │
│  You can change your archetype at any time.          │
│                                                       │
│  Strength      █████████░  9                         │
│  Muscle        ████████░░  8                         │
│  Cardio        ████████░░  8                         │
│  Mobility      ██████░░░░  6                         │
│  Control       ████████░░  8                         │
│  Stamina       █████████░  9                         │
│  Longevity     ███████░░░  7                         │
╰───────────────────────────────────────────────────────╯
```

Exact visuals may follow the existing Svarog TUI style.

---

# 4. Navigation

Supported controls:

```text
← / h       Previous archetype
→ / l       Next archetype
Enter       Select
Esc         Return to previous onboarding step
```

Left/right should wrap:

```text
Boxer ← Lifer
Lifer → Boxer
```

The initially selected archetype is:

```text
Athlete
```

This acts as the general-purpose default.

---

# 5. Archetype presentation

Archetypes are presented with their name, description and seven stat bars. Do not add decorative icons or symbols; the selector should remain compact and text-first.

---

# 6. Stats

Every archetype exposes seven characteristics.

All characteristics are scored from `1` to `10`.

These values describe **training priority**, not the user's current ability.

For example:

```text
Wrestler
Strength: 9
```

means:

> Wrestler-style training strongly prioritizes strength.

It does **not** mean:

> The current user has strength level 9.

## Characteristics

### Strength

Ability to produce force.

Influences loaded movements, pulling, pushing, carries, holds and progression toward harder resistance.

### Muscle

Bias toward gaining or preserving skeletal muscle.

Influences hypertrophy-oriented movements, resistance work and progressive overload.

### Cardio

Bias toward cardiovascular and aerobic fitness.

Influences walking, running where appropriate, continuous movement and cardiovascular bouts.

### Mobility

Bias toward useful range of motion, flexibility and freedom of movement.

Influences mobility drills, dynamic stretching and movement variety.

### Control

Bias toward coordination, balance, stability and precise body movement.

Influences unilateral work, balance, controlled tempo and technically precise movements.

### Stamina

Ability to accumulate and repeatedly perform physical work.

For Svarog specifically, this includes the ability to tolerate **distributed training volume throughout the day**.

High Stamina archetypes should gradually bias toward more accumulated work as user capacity demonstrates that it is appropriate.

### Longevity

Bias toward maintaining useful physical capability over decades.

Influences sustainable resistance training, muscle preservation, aerobic fitness, balance, mobility and avoidance of unnecessarily risky training.

---

# 7. Built-in archetypes

## Character table

| Archetype          | STR | MUS | CAR | MOB | CTL | STA | LON |
| ------------------ | --: | --: | --: | --: | --: | --: | --: |
| **Boxer**          |   6 |   5 |   9 |   6 |   8 |  10 |   7 |
| **Wrestler**       |   9 |   8 |   8 |   6 |   8 |   9 |   7 |
| **Martial Artist** |   7 |   5 |   7 |  10 |  10 |   8 |   8 |
| **Bodybuilder**    |   9 |  10 |   4 |   4 |   6 |   6 |   6 |
| **Runner**         |   4 |   3 |  10 |   5 |   6 |  10 |   8 |
| **Athlete**        |   8 |   7 |   8 |   8 |   8 |   8 |   8 |
| **Gymnast**        |   8 |   7 |   7 |  10 |  10 |   8 |   8 |
| **Yogi**           |   3 |   3 |   4 |  10 |   9 |   5 |   9 |
| **Mover**          |   5 |   5 |   5 |   9 |   9 |   6 |   9 |
| **Thinker**        |   4 |   3 |   6 |   7 |   7 |   6 |   9 |
| **Lifer**          |   7 |   6 |   8 |   8 |   8 |   7 |  10 |

Abbreviations are internal / compact-display forms only:

```text
STR = Strength
MUS = Muscle
CAR = Cardio
MOB = Mobility
CTL = Control
STA = Stamina
LON = Longevity
```

The full names should be used where terminal width permits.

---

# 8. Archetype descriptions

## Boxer

**Short description**

> Fast, conditioned and durable: calisthenics, footwork, core work and very high distributed training volume.

**Training interpretation**

Bias toward:

* push-oriented calisthenics;
* core endurance;
* shadow boxing;
* footwork where appropriate;
* walking / cardiovascular work;
* repeated small sets;
* gradually increasing total daily volume;
* conditioning without excessive equipment dependence.

The Boxer archetype strongly embodies Svarog's philosophy of accumulating physical work across the entire day.

---

## Wrestler

**Short description**

> Powerful and hard to fatigue: pulling, grip, posterior chain, carries, isometrics and explosive full-body strength.

**Training interpretation**

Bias toward:

* pulling;
* rows;
* grip;
* farmer carries / holds;
* posterior-chain work;
* bracing;
* isometrics;
* full-body resistance;
* explosive work where safe;
* substantial conditioning.

---

## Martial Artist

**Short description**

> Lean, mobile and precise: speed, coordination, balance, core control and explosive movement.

**Training interpretation**

Bias toward:

* mobility;
* balance;
* coordination;
* controlled bodyweight movement;
* rotational movement;
* core;
* shadow striking;
* movement precision;
* relative strength;
* moderate conditioning.

Avoid unnecessary hypertrophy bias when other choices provide equal benefit.

---

## Bodybuilder

**Short description**

> Build muscle deliberately through resistance, progressive overload and hypertrophy-focused strength work.

**Training interpretation**

Bias toward:

* resistance exercises;
* muscular tension;
* hypertrophy;
* progressive loading;
* pushing and pulling;
* isolation work where useful;
* controlled repetitions;
* sufficient recovery between repeated muscle-group exposure.

Distributed Svarog sessions should still respect recovery rather than repeatedly exhausting the same muscles.

---

## Runner

**Short description**

> Build a large aerobic engine through movement volume, cardiovascular fitness and durable lower-body endurance.

**Training interpretation**

Bias toward:

* walking;
* running when appropriate and available;
* marching;
* low-level continuous movement;
* aerobic work;
* calf / ankle durability;
* sustainable lower-body endurance;
* very high movement volume over time.

User limitations must override running-specific recommendations.

---

## Athlete

**Short description**

> Be good at everything: balanced strength, muscle, cardio, mobility, coordination and power.

**Training interpretation**

Bias toward balanced development across:

* resistance;
* bodyweight work;
* walking / cardio;
* mobility;
* balance;
* core;
* power where safe;
* general physical preparedness.

This is the default archetype.

No single physical quality should dominate without additional evidence from the user's goals or feedback.

---

## Gymnast

**Short description**

> Master your own body through relative strength, core control, balance, mobility and precise movement.

**Training interpretation**

Bias toward:

* bodyweight strength;
* core;
* controlled push movements;
* controlled pulling where equipment permits;
* balance;
* static holds;
* mobility;
* slow eccentric control;
* progressively more challenging leverage.

Avoid excessive external-load bias unless required by user goals.

---

## Yogi

**Short description**

> Move freely and deliberately through mobility, flexibility, balance, breathing and controlled body awareness.

**Training interpretation**

Bias toward:

* mobility;
* flexibility;
* controlled stretching;
* balance;
* breathing;
* posture;
* recovery;
* low-impact bodyweight movement;
* reducing stiffness created by desk work.

Do not interpret Yogi as "no strength work"; strength may still be used when it supports mobility and control.

---

## Mover

**Short description**

> Strong posture and controlled movement through core work, mobility, balance and low-impact muscular endurance.

**Training interpretation**

Bias toward:

* core;
* glutes;
* posture;
* controlled repetitions;
* slower tempos;
* balance;
* mobility;
* low-impact muscular endurance;
* precise movement.

This archetype captures Pilates-like movement principles without depending on a specific branded methodology or person.

---

## Thinker

**Short description**

> Use movement to improve focus, energy, mood, stress regulation, sleep and cognitive performance.

**Training interpretation**

Bias toward:

* frequent movement breaks;
* walking;
* posture;
* mobility;
* brief cardiovascular activity;
* moderate resistance;
* activities likely to refresh rather than exhaust the user;
* reducing prolonged sedentary periods.

When choosing between equivalent options, prefer the one least likely to impair the user's subsequent knowledge work.

Training should support the working day rather than compete with it.

---

## Lifer

**Short description**

> Stay strong, mobile, aerobically fit and physically independent for as many decades as possible.

**Training interpretation**

Bias toward:

* preserving / increasing skeletal muscle;
* resistance training;
* legs and posterior chain;
* grip and carries;
* aerobic fitness;
* balance;
* mobility;
* sustainable progression;
* maintaining broad physical capability.

Avoid chasing short-term performance at the expense of unnecessary injury risk.

---

# 9. Custom archetypes

The user must also be able to enter a custom archetype.

From the archetype selector, provide a key such as:

```text
/       Enter custom archetype
```

Example:

```text
Custom archetype:
Mike Tyson
```

or:

```text
Custom archetype:
Goku
```

or:

```text
Custom archetype:
a climber
```

The value may be:

* a real person;
* a fictional character;
* an athlete;
* a profession;
* a physical archetype;
* another concept recognizable by the LLM.

The recommender should attempt to infer the intended training characteristics.

If the model cannot confidently understand the reference, use `Athlete` as the behavioral fallback.

Do not fail onboarding.

Persist both values where applicable:

```toml
archetype = "Mike Tyson"
archetype_base = "custom"
```

The original user input should remain available to the recommender.

---

# 10. Changing archetype later

Archetype is normal mutable configuration.

The user must be able to change it after onboarding without repeating onboarding.

For example:

```bash
svarog config archetype
```

or through the existing configuration mechanism.

It should reopen the same archetype selector UI.

The selector footer should explicitly state:

```text
You can change your archetype at any time.
```

Changing archetype affects **future recommendations only**.

It must not:

* delete previous exercise history;
* reset learned capacity;
* reset the database;
* rerun onboarding;
* discard feedback.

Svarog should continue knowing what the user has demonstrated they can actually tolerate.

---

# 11. Prompt architecture

Each built-in archetype should have its own Jinja2 partial.

Suggested structure:

```text
prompts/
  recommender.j2

  archetypes/
    athlete.j2
    boxer.j2
    wrestler.j2
    martial_artist.j2
    bodybuilder.j2
    runner.j2
    gymnast.j2
    yogi.j2
    mover.j2
    thinker.j2
    lifer.j2
    custom.j2
```

The selected archetype determines which partial is inserted into the recommender prompt.

Conceptually:

```jinja2
{% include "archetypes/" ~ archetype_template ~ ".j2" %}
```

Do not duplicate the complete recommender prompt in every archetype file.

Archetype templates contain **biases and interpretation instructions only**.

---

# 12. Base recommender prompt

The base recommender remains responsible for safety, context and actual workload selection.

Conceptually:

```text
You recommend very short physical activities that a user can perform
while an AI coding agent is working.

The user is not following a conventional workout session.
Physical work is distributed across their normal working day.

Use the user's archetype as a long-term training bias, NOT as a literal
workout plan and NOT as evidence of their current fitness.

The user's demonstrated capacity, limitations, equipment, accumulated
daily work and recent behavior always override archetype ambition.

Never assume the user can perform a difficult exercise merely because
their archetype emphasizes it.

Prefer useful work that can be accumulated sustainably over time.
```

Then include:

```jinja2
{% include selected_archetype %}
```

followed by runtime context.

---

# 13. Common archetype helper

Every archetype partial should make the following distinction explicit:

```text
This archetype describes the user's desired long-term direction.

It does not describe their current ability.

Use it primarily to influence:
- exercise selection;
- muscle-group emphasis;
- modality;
- long-term progression;
- distribution of training volume.

Do not use it by itself to determine:
- repetitions;
- duration;
- resistance;
- immediate intensity;
- whether an exercise is safe.
```

This prevents archetype selection from accidentally replacing the removed `Forge intensity` setting.

---

# 14. Example archetype template

Example:

```jinja2
{# archetypes/boxer.j2 #}

## Forge archetype: Boxer

The user's long-term training north star is a boxer.

Bias recommendations toward:
- calisthenics;
- pushing endurance;
- strong and fatigue-resistant core;
- footwork and coordination;
- shadow boxing where practical;
- walking and cardiovascular movement;
- minimal-equipment exercises;
- high accumulated daily work capacity.

A Boxer should gradually become comfortable accumulating many small
pieces of useful physical work throughout the day.

Do not attempt to reproduce an elite boxer's literal training volume.
The user's demonstrated capacity determines actual load.
```

Example:

```jinja2
{# archetypes/lifer.j2 #}

## Forge archetype: Lifer

The user's long-term training north star is lifelong physical capability.

Bias recommendations toward maintaining and improving:
- skeletal muscle;
- usable strength;
- aerobic fitness;
- balance;
- mobility;
- grip;
- lower-body capability;
- physical independence.

Prefer sustainable progress and broad capability over short-term
specialization.

Avoid unnecessary fatigue or risk whose only purpose is maximizing
short-term performance.
```

---

# 15. Custom archetype helper

For custom values:

```jinja2
{# archetypes/custom.j2 #}

## Custom Forge archetype

The user chose:

"{{ archetype_name }}"

If you clearly recognize the person, character, profession or concept,
infer the physical qualities and training philosophy the user is
probably expressing.

Use those inferred qualities only as a long-term training bias.

Do not reproduce a real person's literal training program or assume
the user shares their abilities.

If the reference is unclear or you cannot confidently infer useful
training characteristics, behave as the Athlete archetype.
```

This provides automatic support for:

```text
Mike Tyson
David Goggins
Arnold Schwarzenegger
a climber
a ballet dancer
Batman
Geralt
```

without maintaining first-party templates for them.

---

# 16. Adaptive workload

Removing `Forge intensity` and `Forge interval` means workload becomes learned.

The recommender should progressively infer appropriate load from real behavior.

Examples of useful signals:

### Completed as recommended

```text
recommended: 8
performed: 8
```

Weak positive evidence.

### User increases reps

```text
recommended: 8
performed: 15
```

Evidence that current prescription may be too conservative.

### User decreases reps

```text
recommended: 15
performed: 8
```

Evidence that current prescription may be too aggressive.

### User skips

```text
recommended: 10
performed: skipped
```

Evidence against immediately increasing load.

Do not automatically interpret a single skip as inability. Consider recent context and repeated behavior.

### Pain / limitation feedback

Strong negative evidence.

Immediately override archetype progression for affected movements where appropriate.

---

# 17. Agent runs as opportunities

An agent run should no longer map mechanically to:

```text
every N runs → exercise
```

Instead, each eligible agent event creates a **movement opportunity**.

Conceptually:

```text
agent event
    ↓
movement opportunity
    ↓
consider:
    current day
    recent forge
    accumulated work
    current capacity
    recent skips
    recent modifications
    archetype
    goals
    equipment
    limitations
    ↓
forge or no forge
```

The long-term direction comes from the archetype.

The immediate decision comes from context.

This allows:

```text
Boxer
```

to eventually develop very high daily training volume without requiring the user to select:

```text
intensity = 5
interval = 1
```

during onboarding.

---

# 18. Persistence

Add archetype to user configuration.

Suggested representation:

```toml
[forge]
archetype = "athlete"
```

For custom:

```toml
[forge]
archetype = "custom"
custom_archetype = "Mike Tyson"
```

Built-in archetype IDs should be stable lowercase identifiers:

```text
boxer
wrestler
martial_artist
bodybuilder
runner
athlete
gymnast
yogi
mover
thinker
lifer
custom
```

Display labels are presentation concerns and may change independently.

---

# 19. Archetype metadata

Prefer defining built-in metadata in one structured source rather than scattering it across UI code.

Conceptually:

```rust
Archetype {
    id: "boxer",
    name: "Boxer",
    description: "...",
    strength: 6,
    muscle: 5,
    cardio: 9,
    mobility: 6,
    control: 8,
    stamina: 10,
    longevity: 7,
    prompt_template: "archetypes/boxer.j2",
}
```

This metadata drives:

* onboarding UI;
* settings UI;
* character stats;
* description;
* Jinja partial selection.

There should be one authoritative list.

---

# 20. README-level explanation

The feature should be explainable in approximately this amount of text:

> **Choose your Forge archetype.**
>
> Your archetype is the kind of physical capability you want Svarog to gradually train you toward: Boxer, Wrestler, Athlete, Gymnast, Yogi, Lifer, and others.
>
> It doesn't determine how hard you train today. Svarog learns that from what you actually do.
>
> You can change archetypes at any time.

The core product model is:

```text
Goals
    ↓
What do I want?

Archetype
    ↓
What kind of physically capable person do I want to become?

History + feedback
    ↓
What can I actually handle right now?

Svarog
    ↓
What should I do during this agent run?
```

---

# 21. Acceptance criteria

The implementation is complete when:

* [ ] `Forge intensity` is removed from onboarding.
* [ ] `Forge interval` is removed from onboarding.
* [ ] Archetype selection appears after physical profile / limitation questions.
* [ ] Selector uses Svarog's runtime visual language rather than ordinary onboarding prompts.
* [ ] Left/right arrows cycle through archetypes in-place.
* [ ] Selection wraps from last to first and first to last.
* [ ] `Athlete` is the default.
* [ ] Every archetype displays its title, description and seven stats without decorative symbols.
* [ ] User can select a built-in archetype with Enter.
* [ ] User can provide a custom archetype.
* [ ] Unknown custom archetypes safely fall back to Athlete behavior.
* [ ] Selected archetype is persisted.
* [ ] Archetype can be changed after onboarding.
* [ ] Changing archetype does not erase learned history.
* [ ] Every built-in archetype has its own `.j2` partial.
* [ ] Recommender dynamically inserts the correct partial.
* [ ] Custom archetypes use `custom.j2`.
* [ ] Archetype affects exercise selection / long-term bias, not immediate assumed ability.
* [ ] Repetition changes, skips and other feedback can influence future workload.
* [ ] Agent runs are treated as opportunities rather than a fixed `N`-run interval.
* [ ] The selector tells the user that their archetype can be changed later.
