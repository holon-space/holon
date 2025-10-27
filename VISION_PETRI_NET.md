# Holon: Petri-Net Foundations

> *Everything reduces to five primitives. Everything else is emergent.*

## Origin

This document captures the formal model underlying Holon's task and project management: a Petri-Net + Digital Twin framework for personal productivity. The model was developed in a [design conversation](digital-twins-petri-netze-conversation.md) exploring how industrial process modeling techniques (Petri Nets, Digital Twins) can be adapted for personal project management — and how classical productivity principles (Lean, GTD, Eisenhower, WIP limits) emerge naturally from a small set of primitives rather than being imposed as separate rules.

---

## The Five Primitives

The entire system reduces to five concepts. All higher-level features — prioritization, WIP limits, flow optimization, waste detection, scheduling — emerge from their interaction.

### 1. Token

Something that exists in the world. Has a type, attributes, and a current place.

```yaml
token:
  id: steuererstattung
  type: Monetary
  place: P_monetary      # one place per token type
  attributes:
    value: 2000€
    status: nicht_realisiert   # lifecycle state is an attribute, not a place
```

Tokens are Digital Twins — live representations of real-world entities. A bank account token reflects actual balance. A document token knows where the file lives. The "Self" is a token with attributes like energy, focus, and mental slots. Tokens are created and destroyed dynamically as the real world changes — the system does not conserve tokens.

#### Composite Tokens

A Digital Twin can be a complex object with nested attributes, some of which can be **projected** into independent tokens when a transition needs to reference them directly. This projection is lazy — it only materializes when something actually needs it as an input arc.

```yaml
# The Self token has attributes that can act as independent tokens:
self:
  energy: 0.7           # projectable: transitions can consume self.energy
  focus: 0.8            # projectable: transitions can require self.focus >= threshold
  attention_slots: 3    # projectable: transitions borrow/return slots

# An Organization token contains Person tokens:
engineering_team:
  type: Organization
  members: [alice, bob, carol]   # each projectable as independent Person token
  budget: 50000                  # projectable as Monetary token
```

This means `@[[Engineering Team]]: deploy feature` treats the team as one token (someone from the team does it), while `@[[Alice]]: review PR` projects Alice out as an independent token. Both work simultaneously without contradiction.

### 2. Place

A container for tokens of a given type. Each token type has **one place** — all tokens of that type live there regardless of their attribute values.

```yaml
places:
  - P_document    # all documents, any status
  - P_person      # self and other people
  - P_monetary    # budgets, invoices, refunds
  - P_knowledge   # information, answers, research results
  - P_resource    # URLs, files, external references
```

Attribute-based filtering (e.g., "only reviewed documents") is handled by **guards** on transitions, not by splitting into multiple places. This avoids the stranded-token problem where a transition that accepts any document cannot find tokens that happen to be in a status-specific sub-place.

Lifecycle phases like `draft → reviewed → published` are not modeled as separate places. They are **projections** — queryable views that visualize how a chosen attribute evolves across transitions (see [Projections](#projections-views-on-the-flat-net)).

### 3. Transition

An action that consumes input tokens and produces output tokens. Takes time. May have guards (preconditions).

```yaml
transition:
  id: fill_tax_form
  inputs: [self, income_statement, knowledge_of_deductions]
  outputs: [self, completed_form]       # self is returned (borrowed, not consumed)
  duration: 45min
  guards:
    - self.energy >= 0.3
    - self.focus >= 0.2
  effects:
    - self.energy -= 0.15               # executor's attributes can change
    - self.attention_slots -= 1         # during firing; restored on completion
```

There is no separate "executor" concept. The executor is simply a **Person token in the input/output arcs** with a borrowed semantic — it goes in, comes back out, but its attributes may change (energy drained, slot occupied). This keeps the primitive count at five: the `@` syntax in the [task layer](#the-executor-model) is sugar for "put this Person token in the input/output arcs instead of self."

Transitions are the atoms of work. A composite transition (project) is a sub-net — a Petri net inside a transition, giving the model fractal structure.

### 4. Time

A continuously flowing resource. Time creates urgency through two mechanisms:

- **Discounting**: Future value is worth less than present value. A euro received today is worth more than a euro received in six months.
- **Deadlines**: Some transitions have hard deadlines. Missing them triggers penalty tokens (fines, lost opportunities).

Time is not an artificial motivator. It is the medium through which opportunity cost manifests.

### 5. Objective Function

A function over all token attributes at time *t*, yielding a scalar. The system maximizes this function over time.

```yaml
objective_function:
  components:
    - sum(monetary_tokens) × discount(t)
    - self.health × health_weight
    - self.relationships × relationship_weight
    - sum(utility_tokens)
  constraints:
    - self.energy > 0.1
    - self.health > 0.5
```

The weights are personal — they encode what matters to this specific person. The function is **data, not code**: stored as a Holon block, editable, refinable over time.

---

## What Emerges

Every classical productivity concept reconstructs from these five primitives:

| Concept | How It Emerges |
|---------|---------------|
| **Cost of Delay** | Time-value of money + forgone utility. Not artificial motivation — real opportunity cost. |
| **WSJF** | CoD / Duration. The optimizer naturally sequences by this ratio. |
| **WIP Limits** | Context-switch transitions cost time but produce zero value. Fewer parallel tasks = fewer switches = more net value. |
| **Flow / Pomodoro** | Discounting makes faster completion strictly better. The Self DT's energy/focus dynamics produce natural work/break rhythms (see [Flow Dynamics](#flow-dynamics-and-emergent-work-rhythms)). |
| **Pull** | Transitions fire only when all inputs are present. Work is pulled when capacity exists. |
| **Small Batches** | Partial value realization + discounting makes early completion of sub-parts strictly better. |
| **Priority** | Gradient magnitude of the objective function with respect to completing a transition. |
| **Urgency** | Steepness of the gradient near a deadline (penalty function kicks in). |
| **Eisenhower Matrix** | Important = high absolute contribution to objective function. Urgent = steep gradient near deadline. |
| **2-Minute Rule** | Tiny transitions that free a mental slot produce disproportionate value (slot recovery > time cost). |
| **7 Wastes (Muda)** | All are transitions with duration > 0 and value = 0, or tokens stuck in intermediate places. |

### The 7 Wastes Reconstructed

1. **Transport** — Transitions that move tokens between formats without adding value (reformatting a document nobody requested)
2. **Inventory** — Tokens sitting in intermediate places (half-finished work binding mental slots)
3. **Motion** — Search transitions with zero value output. Digital Twins eliminate this: location is always known.
4. **Waiting** — Transitions blocked on external inputs. Visible as: time passes, no value produced.
5. **Overproduction** — Output tokens that are never consumed by any downstream transition.
6. **Over-processing** — Transitions with longer duration than necessary for the same output.
7. **Defects** — Faulty output tokens requiring repair transitions (duration > 0, value = 0).

---

## The Self Digital Twin

The most important token in the system. Models the person using Holon.

```yaml
token: self
type: Person
attributes:
  energy:
    value: 0.7
    max: 1.0
    decay: -0.05/hour (awake)
    restore: sleep, exercise, rest

  focus:
    value: 0.8
    max: 1.0
    ramp_time: ~10-15min         # time to reach deep focus on a single task
    consumed_by: deep_work transitions

  flow_depth:
    value: 0.0                   # 0 = not in flow, 1.0 = deep flow
    builds_when: sustained focus on single task, no context switches
    decays_when: context switch or break
    signals:                     # observable proxies for flow_depth
      - window_switch_rate       # OS-level: fewer switches = deeper flow
      - browser_tab_switches     # browser plugin: tab hopping = shallow focus
      - app_category_penalty     # negative weight for Youtube, Instagram, Reddit, ...
      - typing_cadence           # sustained typing = likely in flow
  peripheral_awareness:
    value: 0.8
    max: 1.0
    inversely_correlated: flow_depth   # deeper flow = less awareness of surroundings

  mental_slots:
    capacity: 7          # Miller's Law (7 ± 2)
    current_load: 3      # Open projects/loops

  health:
    value: 0.85
    max: 1.0
    decay: -0.002/day (without maintenance)

  relationships:
    partner: 0.9
    children: 0.85
    friends: 0.6
```

### Mental Slots

The key cognitive resource. Based on Miller's Law and the Zeigarnik effect (unfinished tasks stay in working memory).

**Effectiveness curve:**

| Slots Used | Effectiveness |
|-----------|--------------|
| 0–2 | 1.00 — clear head, full capacity |
| 3–4 | 0.95 — still good |
| 5 | 0.85 — noticeable drag |
| 6 | 0.70 — effortful |
| 7 | 0.50 — at limit |
| 8+ | 0.30 — overloaded, errors, forgetting |

**What occupies a slot:**
- Any in-progress project or task (started but not completed)
- Anything with an unresolved dependency ("waiting for X")
- Anything with a nearby deadline
- Open emotional loops (unresolved conversations, decisions)

**What frees a slot:**
- Completing something (even a small milestone)
- Writing it down with a clear next action (GTD's "capture")
- Delegating with a reliable follow-up system
- Deciding "not doing this" and archiving it

**Why this matters for the optimizer:**
Without mental slots, the optimizer might interleave projects freely (A1, B1, A2, B2...) since total duration is the same. With mental slots, keeping two projects open simultaneously reduces effectiveness on both. The optimizer naturally prefers: finish A, then start B — or at least: finish a meaningful chunk of A before touching B.

This also explains why breaking projects into completable milestones is valuable even when the full value only arrives at project completion: each completed milestone frees a slot, improving effectiveness on everything else.

### Attention Slots vs. Full Self Consumption

Transitions do not consume the Self token entirely. Instead, they borrow **attention slots** — a projected facet of Self:

```yaml
T: "Deep work on tax form" (duration: 3h)
  inputs: [self.attention_slots >= 1, self.energy >= 0.3, self.focus >= 0.5]
  during_firing: self.attention_slots -= 1, self.energy -= rate_per_minute
  on_complete: self.attention_slots += 1

T: "Quick review of AI result" (duration: 2min)
  inputs: [self.attention_slots >= 0]   # doesn't even need a full slot
  during_firing: self.energy -= 0.02
```

The Self token is never fully consumed. During a 3h task, you still have free slots for short tasks. The system can suggest: "You have 5 minutes between Pomodoros — here are 3 quick tasks that fit."

### Flow Dynamics and Emergent Work Rhythms

Rather than prescribing Pomodoro (25 min work / 5 min break) as a rule, work/break patterns **emerge** from the Self DT's dynamics.

**During sustained work on a single task:**
- `energy` decays at a constant rate per minute
- `focus` increases initially (ramp-up over ~10-15 min), then plateaus
- `flow_depth` increases while focus is high and no context switches happen
- `peripheral_awareness` **decreases** as flow_depth increases — this is the tunnel vision effect

**During a break:**
- `energy` partially restores (diminishing returns)
- `focus` drops to baseline, `flow_depth` resets
- `peripheral_awareness` restores to baseline

**During a context switch (different from a break — switching to another task):**
- `energy` drains *more* than sustained work (switching cost)
- `focus` drops to near-zero, must ramp up again
- `flow_depth` resets
- `peripheral_awareness` briefly spikes (you see the landscape), then drops as you refocus

**What emerges:**

The system computes an **optimal break point** by balancing opposing forces:

1. **Value of continued flow**: high focus = high productivity = more value per minute
2. **Risk of tunnel vision**: low peripheral_awareness = missing important things (urgent messages, better-priority tasks becoming available, physical needs)
3. **Energy depletion**: working past low energy produces errors (defect transitions — waste)

The break point is where: `marginal_value_of_one_more_minute < cost_of_reduced_awareness + cost_of_energy_depletion`

For most people, this naturally produces ~25–50 minute work blocks — focus ramp-up takes ~10 min, flow stabilizes around 15–20 min, peripheral awareness drops below a useful threshold around 30–50 min, and energy depletion becomes noticeable around 45–60 min.

But the exact numbers differ per person. The system learns from observed behavior (when does the user take breaks? when do they start making mistakes?) and offers **tunable parameters**:

| Parameter | What it controls | Personal variation |
|---|---|---|
| `energy_decay_rate` | How quickly you tire | Varies by sleep, fitness, time of day |
| `focus_ramp_time` | How long to get into a task | Varies by task type, environment |
| `awareness_decay_rate` | How quickly tunnel vision sets in | Personality trait |
| `awareness_threshold` | When to suggest interrupting flow | User preference |
| `break_restore_rate` | How much rest helps | Varies by break activity (walk vs. scroll phone) |

Pomodoro (25/5) is one point in this parameter space. The system doesn't prescribe it — it discovers your personal optimal rhythm, which might be Pomodoro, or 45/10, or varying by time of day and task type.

**During breaks, the system can surface:**
- Completed `@ai` results awaiting review (Trust Level 1 confirmations)
- 2-minute tasks with high WSJF (slot recovery value)
- Pending notifications from delegated tasks

**Detection of "too deep in flow":** When `peripheral_awareness` drops below threshold AND there are pending high-priority items, the system gently suggests a break — not because "25 minutes are up" but because there's something you should see.

---

## Tasks as Transitions: The Dynamic Integration Model

The Petri net is not a static model defined upfront. It grows dynamically as the user creates tasks in the outliner. Every task block materializes into a transition, and the engine continuously re-evaluates which transitions are enabled and how they rank.

### Tasks ARE Transitions (Not Tokens)

A task is an action that transforms the world — it is a transition, not a token:

| Block Property | Petri Net Concept |
|---|---|
| `content` | Transition name/label |
| `task_state` (TODO/DOING/DONE) | Whether the transition is unfired / firing / fired |
| `priority` (A/B/C) | Feeds into CoD → WSJF ranking |
| `deadline` | Deadline penalty function on the transition |
| `duration` (new property) | Transition duration for WSJF |
| `executor` (`@person`, `@ai`, or implicit self) | Who performs the transition |
| Child task blocks | Sub-net (composite transition) |

Tokens are the **things** tasks act on — documents, invoices, people, devices, knowledge. Some tokens are implicit (the `self` token is always present); others are referenced explicitly via `[[wiki links]]` in the task text.

### The Executor Model

"Executor" is not a new primitive — it is syntactic sugar for **which Person token appears in the input/output arcs** of a transition. The `@` prefix determines this:

| Syntax | Who is in the arcs? | Self token involved? | Petri Net effect |
|---|---|---|---|
| `task` (no @) | Self (borrowed) | Yes — attention slot borrowed, energy drained | Standard self-transition |
| `@[[Person]]: task` | That person (borrowed) | No — Self is freed, waiting_for token created | Delegation sub-net (see below) |
| `@ai: task` | AI agent token (borrowed) | No — Self freed | Agent fires asynchronously |
| `@anyone: task` | Any Person (unbound) | No — any team member's Self can bind | Team pool (later phases) |

The absence of `@` means "I do it myself" — the Self token is placed in the input/output arcs. `@[[Kat]]` means Kat's Person token replaces Self in those arcs. In both cases the Person token is **borrowed**: it appears in both inputs and outputs, so it's returned after the transition fires, but its attributes may change (energy drained, knowledge gained).

The `@` prefix is borrowed from messaging applications where it means "mention/direct to." It works naturally for both people (`@[[Kat]]`) and AI agents (`@perplexity`, `@claude`). The system opens a person/agent picker popup when the user types `@`.

### Delegation and the Waiting-For Pattern

`@[[Person]]: task` creates a **delegation sub-net** — one of the most valuable patterns in the system, automating GTD's notoriously hard-to-maintain "waiting for" list:

```
@[[Kat]]: Zeitplan erstellen

Materializes as:
  T1: "Delegate to Kat" (fires immediately, duration ≈ 0)
    inputs:  [self @ active]
    outputs: [self @ active, waiting_for_kat_zeitplan @ P_waiting]

  T2: "Kat delivers Zeitplan" (fires when Kat's DT syncs a response)
    inputs:  [waiting_for_kat_zeitplan @ P_waiting]
    outputs: [zeitplan @ P_document]
```

T1 fires when you send the request (message, email, in-person). T2 fires when Kat's Digital Twin detects a response through any channel (WhatsApp, email, Todoist assignment completion). The waiting_for token is visible in Orient mode: "You have 3 pending delegations."

**Distinguishing delegation from talking about someone:** `@[[Kat]]: task` means "Kat does this" (delegation). `[[Kat]]` in task text without `@` prefix and `:` is context — a tag, not a dependency. For example: `Überlegen, was ich [[Kat]] zum Geburtstag schenken könnte` — Kat is the topic, not the executor.

### Questions and Information Flow

Questions are a distinct transition type, recognizable by the `?` prefix. They produce **Information tokens** — knowledge that may be needed by downstream transitions.

```
? Has Finanzamt charged?

Materializes as:
  T: "Resolve question: Finanzamt charged?"
    inputs:  [self @ active]
    outputs: [info_finanzamt_charged @ P_knowledge]
             info_finanzamt_charged.confidence = <depends on resolution method>
```

#### Multi-Source Resolution

A question can often be answered through multiple paths. This is an **OR-join** — multiple transitions that all produce the same output token, but only one needs to fire:

```
? Has Finanzamt charged?
  via: [[Business Bank Account]]   # check DT directly
  via: @perplexity                 # quick web research
  via: @[[Tax Consultant]]         # ask expert

Materializes as:
  T1: "Check bank account DT"          → info {confidence: 0.95}
  T2: "Ask Perplexity"                 → info {confidence: 0.6}
  T3: "Ask tax consultant"             → info {confidence: 0.95}
```

The system can suggest `via:` routes automatically based on rules:

```yaml
rule:
  when: question AND tag contains "tax" OR "Steuer"
  add_via: @perplexity (confidence: 0.6)
  add_via: @[[Tax Consultant]] (confidence: 0.95)
```

#### Confidence

Information tokens carry a `confidence` attribute — how reliable the knowledge is. This is not probability ("70% chance X happened") but epistemic confidence ("I'm 70% sure my answer is correct").

High-confidence answers let downstream transitions fire. Low-confidence answers can trigger a **confirmation transition**: get a quick answer from Perplexity (confidence 0.6), then confirm with a human expert only if confidence is below threshold.

```
T_perplexity: → info {confidence: 0.6}
T_confirm:    guard: info.confidence < 0.8
              → info {confidence: 0.95, confirmed_by: @[[Tax Consultant]]}
```

The threshold for "good enough" can be per-domain or per-user. Financial decisions might need 0.9+. Casual research might be fine at 0.5.

### Bare Nouns as Tokens (Checklists)

Children of a transition can be **tokens** rather than **sub-transitions**. The distinguishing factor: children with verbs are transitions; bare nouns are tokens.

```
- [ ] Für Urlaub einpacken              # composite transition (verb: einpacken)
  - [ ] Zahnbürste                      # token (input to "einpacken")
  - [ ] Socken                          # token
  - [ ] Ibuprofen                       # token
```

The parent task "einpacken" is a transition. The children are its input tokens — things to pack. Checking off "Zahnbürste" means that token has been consumed by the transition (it's in the bag).

This pattern applies to shopping lists, packing lists, ingredient lists — any case where the parent describes an action and children describe the things it acts on.

**Restriction:** A task's children should be either all tokens (checklist) or all transitions (sub-net), not mixed. In practice this is natural — a packing list doesn't contain sub-tasks with verbs, and a project plan doesn't contain bare nouns. The rare exception (`Hotel buchen` mixed into a packing list) should be a sibling under a common parent rather than a child of the packing task.

### Dependencies: Graceful Uncertainty

Dependencies between tasks are **not required**. The system handles tasks at multiple levels of dependency information:

**Layer 0 — No explicit ordering (default).** Sibling tasks under a parent are treated as an unordered set. All are enabled simultaneously. WSJF decides what to suggest first. List position acts as a **tiebreaker**: when two tasks have identical WSJF scores, the one listed first wins. This is free information that costs nothing to capture.

**Layer 1 — Explicit sequential dependency.** The `>` prefix on a task means "depends on the previous sibling":

```
- [ ] Collect income statement
- >[ ] Fill form                    # blocked until previous is done
- >[ ] Submit                       # blocked until previous is done
```

This creates a chain. No `>` = parallel. `>` = sequential. One character of overhead, only when you actually want ordering.

**Layer 2 — AI-inferred dependencies.** An LLM can look at a task list and suggest: "Fill form probably needs Collect income statement first — add dependency?" One-click confirm. This is optional enrichment, never auto-applied.

The key principle: **the system is useful at every layer.** Layer 0 gives WSJF ranking. Layer 1 adds dependency enforcement. Layer 2 adds intelligent suggestions. Each is opt-in.

### Two-Layer Model: Transition States vs. Token Places

Where do task lifecycle states (TODO/DOING/DONE) live? They are **not** places in the Petri net. They are derived properties of transitions.

In a Petri net, places hold **tokens**, not transitions. Transitions **fire**. The task lifecycle maps to transition state:

| task_state | Petri Net meaning |
|---|---|
| `TODO` (backlog) | Transition exists, input arcs not satisfied |
| `TODO` (ready) | Transition is **enabled** — all input arcs satisfied |
| `DOING` | Transition is **firing** (consuming duration) |
| `DONE` | Transition has **fired** — outputs produced |

The engine's `enabled()` function distinguishes backlog from ready. The `history` records which transitions have fired (done). These are computed, not stored.

**Places hold deliverables** — the things tasks produce and consume. Each task, when it fires, produces a completion token in a task-specific output place:

```
T1: "Collect income statement"
  inputs:  [self @ active]
  outputs: [self @ active, t1_deliverable @ t1_complete]

T2: "Fill tax form" (depends_on: T1)
  inputs:  [self @ active, t1_deliverable @ t1_complete]
  outputs: [self @ active, t2_deliverable @ t2_complete]
```

Each dependency creates a unique place that acts as a handoff point. Routing is structural — encoded in arc topology, not runtime filtering.

**Independent tasks** (no dependencies) only consume `self`:

```
T_standalone: "Buy groceries"
  inputs:  [self @ active]
  outputs: [self @ active]
```

These are all enabled simultaneously and the engine's WSJF ranking decides which to suggest first.

```
TRANSITION LAYER (tasks — derived states, what the user sees):
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ backlog  │  │  ready   │  │  doing   │  │   done   │
│ (not     │  │ (enabled │  │ (firing) │  │ (fired)  │
│ enabled) │  │  by eng) │  │          │  │          │
└──────────┘  └──────────┘  └──────────┘  └──────────┘

TOKEN LAYER (deliverables — actual Petri Net places, what the engine uses):
   [self @ active]
   [t0_result @ t0_complete]
   [t1_dep    @ waiting]
```

### Net Materialization

The Petri net is **derived from the block tree**, not hand-written:

```
User creates block: "* TODO [#A] File tax return"
  with DEADLINE: <2026-03-15>
  with child: "** TODO Collect income statement"
  with child: "** >TODO Fill form"
  with child: "** >TODO Submit"
```

This materializes into:

```
Composite Transition: "File tax return"
  Sub-net:
    T1: collect_income_statement (no deps → immediately enabled)
    T2: fill_form (input: T1's completion token)
    T3: submit (input: T2's completion token)
```

When a user creates a new task block, a new transition is added to the net. The engine re-evaluates `enabled()` and `rank()` — WSJF scores update.

When a user marks a task DONE, the corresponding transition fires, output tokens move to their completion places, downstream transitions may become enabled, and mental slot count decreases.

### Token Conservation: Relaxed

Unlike classical Petri nets, this system does **not** conserve tokens. Tasks routinely create new things ("write a document" creates a document token) and consume things ("pay invoice" removes an obligation token). Tokens are created and destroyed as the real world changes.

---

## Task Syntax: From Text to Petri Net

The system parses task text into Petri Net structures using a combination of lightweight syntax markers, a verb dictionary, and optional AI enrichment. The design goal: **as convenient as a normal text-based task manager**, with a few markers that unlock the full power of the formal model.

### Syntax Reference

| Marker | Meaning | Example |
|---|---|---|
| `@[[Person]]: text` | Delegation (creates waiting_for) | `@[[Kat]]: Zeitplan erstellen` |
| `@agent: text` | AI/automation executor | `@perplexity: What is X?` |
| `? text` | Question (produces Information token) | `? Has Finanzamt charged?` |
| `> task` | Depends on previous sibling | `>Fill form` |
| `verb [[Token]]` | Transition operating on token | `erstellen [[Rechnung DBG]]` |
| `#[[Context]]` | Tag (NOT a dependency) | `task #[[Finanzamt]]` |
| `via: [[Source]]` | Resolution method for questions | `? X via: @perplexity` |
| bare noun (under parent) | Token input to parent transition | `Zahnbürste` |
| URL (standalone) | Resource token + implicit "evaluate" | `https://github.com/...` |
| `⏫ 🔼 🔽` | Priority → CoD weight | existing convention |
| `[deadline:: date]` | Deadline → penalty function | existing convention |
| `🔁 every X` | Periodic transition | `Geburtstag 🔁 every year` |

### The Grammar

Many tasks follow a simple structure: `@[[Subject]]: predicate [[Object]]`, where the `@[[Subject]]:` is optional (absence implies self as executor).

| Task | Parsed as |
|---|---|
| `Rechnung DBG erstellen` | predicate=erstellen, object=Rechnung DBG |
| `@[[Kat]]: Zeitplan erstellen` | subject=Kat, predicate=erstellen, object=Zeitplan |
| `? Has Finanzamt charged` | type=question, topic=Finanzamt |
| `Fix Garden Behave tests` | predicate=fix, object=Garden Behave tests |
| `Zahnbürste` | bare noun → token |

This grammar handles ~70% of real-world tasks. The remaining 30% — complex subordinate clauses, SOP navigation steps, time-annotated tasks — are treated as opaque transition labels. The verb dictionary still picks up recognized verbs; the syntax markers (`@`, `?`, `>`, `[[link]]`) work regardless of sentence structure.

### Three Roles of `[[References]]`

A `[[reference]]` in task text can play three distinct roles:

| Role | Syntax | Example | Petri Net effect |
|---|---|---|---|
| **Executor** | `@[[Person]]: task` | `@[[Kat]]: Zeitplan erstellen` | Person token as executor, delegation sub-net |
| **Object** | `verb [[Token]]` | `erstellen [[Rechnung DBG]]` | Token is input/output of transition |
| **Context** | `#[[Tag]]` or bare `[[link]]` | `Überlegen, was ich [[Kat]] schenke` | Tag on transition, NOT a dependency |

**Safe default:** A bare `[[reference]]` without `@:` prefix or recognized verb-object position is treated as **context** (tag). This prevents false dependencies — a task is never incorrectly blocked because of a misinterpreted reference. Explicit syntax (`@`, verb-object grammar) is required to create actual input arcs.

### Verb-to-Operation Mapping

The verb in the task text reveals transition semantics without AI:

| Verb pattern | Petri Net meaning |
|---|---|
| `erstellen` / create | Token: `not_exists` → `exists` |
| `recherchieren` / research | Token: `unknown` → `researched` |
| `bestellen` / order | Token: `wanted` → `ordered` |
| `überweisen` / pay | Token: `unpaid` → `paid` |
| `abschicken` / send | Token: `draft` → `sent` |
| `besprechen` / discuss | Token gains `discussed` attribute |
| `checken` / check | Produces Information token |
| `fragen` / ask | Question transition, may involve person |

A dictionary of ~30 German + English verbs with their place-transition mappings handles most cases via classical NLP (lemmatization + dictionary lookup). The dictionary is extensible per user.

### Integration with External Tools

The syntax is designed to work within the constraints of external systems like Todoist, where tasks are one-line text with labels and priority levels:

| Holon concept | In Todoist | In Holon (native) |
|---|---|---|
| Person reference | `@Kat` (Todoist label) | `@[[Kat]]` |
| Priority | p1/p2/p3/p4 | ⏫🔼🔽 or A/B/C |
| Deadline | Due date | `[deadline:: date]` |
| Question | `?` prefix or `@question` label | `?` prefix |
| Delegation | `@Kat` label or assigned | `@[[Kat]]:` |
| Sequential dep | Not natively supported → tracked in Holon overlay | `>` prefix |
| Duration | `@30min` label or description | Property |

Todoist's `@context` labels map naturally to the `@Person` syntax. Users who already use `@home`, `@office`, `@Kat` in Todoist are halfway to the Holon model. Holon's sync layer maps Todoist `@Kat` → internal `[[People/Kat]]`. The user never types double brackets on their phone.

This means Holon provides WSJF ranking, waiting_for tracking, and dependency awareness even when all tasks live in Todoist. The integration-first approach eliminates migration cost: users keep Todoist for quick capture and get Holon's intelligence on top.

**Waiting_for tracking with Todoist tasks:** The PN state in Holon is the source of truth. When Holon detects a delegation pattern (task with `@Kat` label in Todoist), it creates the waiting_for token in its own PN model. Holon can optionally sync this state back to Todoist by adding a `@waiting_for` label and/or a comment ("Delegated to Kat on Feb 21 — awaiting response"). When Kat completes the task in Todoist (or the user marks it done), the Todoist DT sync fires the resolution transition and clears the waiting_for token. Dependencies and WSJF ranking live in Holon's overlay; Todoist sees at most an extra label.

### Progressive Enrichment Pipeline

Task interpretation runs through a pipeline of enrichment stages, each optional:

```
Raw text
  → Syntax parser (deterministic, always runs)
      Handles: @, ?, >, [[links]], priorities, deadlines, durations
  → Verb dictionary (deterministic, always runs)
      Handles: ~30 verbs → transition types, ~70% coverage
  → AI enrichment (optional, suggestion-only, user confirms)
      Handles: suggest [[links]] for plain-text nouns
               classify ambiguous tasks
               infer likely dependencies
               suggest via: routes for questions
```

The core principle: **the system is fully functional with zero AI. AI adds intelligence, not correctness.**

| Feature | Without AI | With AI |
|---|---|---|
| Task parsing | Syntax markers + verb dictionary | + NLU for ambiguous cases |
| Token identification | Explicit `[[links]]` only | + suggests links for plain-text nouns |
| Dependencies | Only explicit (`>` prefix) | + infers likely dependencies |
| Question routing | Only explicit `via:` | + suggests resolution paths from context |
| WSJF ranking | Priority + deadline + duration + list position | + learns from user behavior |
| Delegation detection | Only `@[[Person]]:` syntax | + detects "fragen ob", "tell X to" |

### Annotation Strategy: Layered, Not Mandatory

| Approach | Coverage | Friction |
|---|---|---|
| Parse `@`, `?`, `>`, `[[links]]` | ~35% | Zero (already part of typing) |
| + verb→operation mapping | ~65% | Zero |
| + UI nudge to add `[[link]]` | ~85% | Very low (bracket pair around existing noun) |
| + LLM batch suggestion | ~95%+ | Near zero (one-click confirm) |

Each layer is independent and additive. A task with no markers and no recognized verb simply becomes a self-transition — still valid, still WSJF-ranked, just without rich token semantics.

---

## The Flat Net Model: One Place Per Token Type

### Why Not One Place Per Attribute Value?

An intuitive approach is to create places for each discrete attribute value: `P_draft`, `P_reviewed`, `P_published` for documents. But this creates the **stranded-token problem**:

```
T_review:  post: {status: "reviewed"}  → token moves to P_reviewed
T_submit:  pre:  {status: "reviewed"}  → draws from P_reviewed ✓
T_archive: pre:  {type: "document"}    → draws from... P_draft only? ✗
```

T_archive accepts ANY document. But once T_review fires, the document is in P_reviewed — a place T_archive has no input arc from. The token is stranded despite having the right characteristics.

Hierarchical places don't solve this either: attribute dimensions like `status` and `author` can overlap arbitrarily and don't form a single hierarchy.

### The Solution: Flat Net + Guards

Each token type gets **one place**. All attribute filtering happens via runtime guards on transitions:

```yaml
places: [P_document, P_person, P_monetary, P_knowledge, P_resource]

T_review:
  input:  {bind: doc, place: P_document, guard: {status: "draft"}}
  output: {from: doc, place: P_document, set: {status: "reviewed"}}

T_submit:
  input:  {bind: doc, place: P_document, guard: {status: "reviewed"}}

T_archive:
  input:  {bind: doc, place: P_document}  # no guard — accepts any document
```

The token never leaves P_document. Its `status` attribute changes, but its place doesn't. All three transitions draw from the same place. Guards determine which specific tokens each transition can consume.

| Structural (place) | Runtime (guard) |
|---|---|
| "This transition operates on documents" | "Specifically, reviewed documents" |
| "This transition consumes self" | "Specifically, self with energy >= 0.3" |
| "This transition needs a budget token" | "Specifically, one with value >= 1000" |

### What You Lose, What You Keep

You lose the ability to see from the net topology alone that "review must happen before submit" — that ordering is implicit in the guards. But for a dynamic system where users create transitions at runtime, this is the right trade-off.

You keep: WSJF ranking, dependency enforcement (through guards on completion tokens), WIP visibility, mental slot tracking, and simulation capability. The engine evaluates all enabled transitions (guard-passing bindings), ranks by objective delta / duration, and suggests the best one.

---

## Projections: Views on the Flat Net

The flat net (one place per token type) is correct but doesn't show lifecycle phases visually. **Projections** solve this: they are queryable views that filter the net by a token type and an attribute, then present transitions as if they were a linear flow through attribute values.

### What a Projection Is

A projection is defined by: a token type, an attribute to project onto, and optionally a filter:

```yaml
projection:
  name: "Document Review Pipeline"
  token_type: document
  attribute: status
  filter: { author: "Ben" }    # optional subset
  phases:                       # ordered attribute values
    - draft
    - reviewed
    - submitted
```

The projection shows only transitions that have guards or postconditions on `status` for documents, arranged as a pipeline:

```
┌───────┐  T_review  ┌──────────┐  T_submit  ┌───────────┐
│ draft │───────────→│ reviewed │───────────→│ submitted │
└───┬───┘            └────┬─────┘            └───────────┘
    │                     │
    └──── T_archive ──────┘──→ (cross-cutting: available at every phase)
```

Transitions that don't constrain the projected attribute (like T_archive) appear as side-exits available from any phase.

### Why Projections, Not Hierarchical Places

Attribute dimensions overlap arbitrarily. "Documents with status=reviewed" and "documents with author=Ben" are cross-cutting concerns that don't form a hierarchy. With projections, you can have both simultaneously — they're just two different lenses on the same flat set of tokens.

| Approach | Structure | Overlapping dimensions |
|---|---|---|
| One place per attribute combo | Exponential explosion | Must pre-commit to all dimensions |
| Hierarchical places | Tree | Must pick ONE primary dimension |
| Flat + projections | One place per type | Any dimension, any time, any combination |

### Common Projections

| Projection | token_type | attribute | Shows |
|---|---|---|---|
| **Task Kanban** | task | task_state | TODO → DOING → DONE |
| **Document Lifecycle** | document | status | draft → reviewed → published |
| **Invoice Pipeline** | monetary | payment_status | created → sent → paid |
| **Sprint Board** | task | sprint | Backlog → Sprint 42 → Done |

### Projections as SOPs

A projection filtered by responsible entity and topologically sorted produces a **Standard Operating Procedure**: the step-by-step sequence of transitions that entity A needs to follow. Steps are ordered by causal dependencies (respecting the net's arc structure), with conditional branches where guards create alternative paths.

### Implementation

Projections are **queries** (PRQL or GQL), not structural elements of the net. They can be defined as Holon blocks and rendered as Kanban boards, flowcharts, or step-by-step lists. The same underlying flat net supports all of them simultaneously.

---

## Digital Twins and MCP Integration

### Integration-First Architecture

Holon is not a replacement for existing tools — it is an intelligence layer on top of them. Users keep Todoist for quick capture, JIRA for work tickets, Google Calendar for scheduling. Holon deeply integrates with these systems and adds formal reasoning: dependency tracking, WSJF prioritization, delegation management, and Digital Twin synchronization.

This means:
- **Zero switching cost.** Connect Todoist → immediately see WSJF-ranked tasks.
- **Gradual migration.** Start with all tasks in Todoist. Create private/complex tasks natively in Holon when desired.
- **Unified view.** Personal tasks (Todoist) + work tickets (JIRA) + calendar events + native Holon tasks, all in one WSJF-ranked list.
- **Privacy where needed.** Tasks in Todoist live on Todoist's servers. Tasks created natively in Holon stay local (CRDT sync between personal devices only).

### Tokens as Digital Twins

Every token can be backed by a Digital Twin — a live connection to the real-world entity it represents:

| Token Type | Digital Twin Source | Sync Mechanism |
|-----------|-------------------|----------------|
| Bank account | Banking API / CSV import | Periodic pull |
| Calendar event | Google Calendar / iCal | Webhook |
| Email | Gmail API | Webhook |
| Document | Filesystem watcher | File events |
| Todoist task | Todoist API | Webhook + polling |
| JIRA issue | JIRA API | Webhook |
| Self (energy) | Manual input / wearable | Pull / manual |
| Web resource | Browser plugin | Event tracking |

The twin keeps the token's attributes synchronized with reality. When an email arrives, the corresponding token's attributes update automatically. When a Todoist task is completed externally, the transition fires in the Petri net.

### MCP Tools as Atomic Transitions

Every MCP tool maps to an atomic transition:

```
MCP Tool "todoist.complete_task"
  ↓
Transition: complete_task
  inputs: [task at P_task, guard: {status: "in_progress"}]
  outputs: [task at P_task, set: {status: "done"}]
  duration: ~0 (tool execution)
  side_effect: API call via MCP
```

Composite transitions are sub-nets: a "do taxes" project contains transitions for each step, some of which may themselves be MCP tool invocations.

### MCP-Twin Binding Architecture

MCPs are "dumb pipes" — they expose operations and data. Digital Twins are the state/adapter/normalization layer:

```
┌────────────────────────────────────────────────────┐
│              UNIFIED PETRI NET                      │
│                                                     │
│  Transitions reference tokens by type + attributes  │
│  Tokens have places and attributes                  │
└──────────────────────┬─────────────────────────────┘
                       │
          ┌────────────┴────────────┐
          │                         │
   ┌──────▼──────┐          ┌──────▼──────┐
   │ Digital Twin │          │ Digital Twin │
   │  (Todoist)   │          │   (Gmail)    │
   │              │          │              │
   │ State map    │          │ State map    │
   │ Op map       │          │ Op map       │
   │ Sync policy  │          │ Sync policy  │
   └──────┬───────┘          └──────┬───────┘
          │                         │
   ┌──────▼──────┐          ┌──────▼──────┐
   │  MCP Server  │          │  MCP Server  │
   │  (Todoist)   │          │   (Gmail)    │
   └──────────────┘          └──────────────┘
```

Three sync strategies per twin:
- **Pull**: Twin queries MCP on demand
- **Periodic**: Twin polls MCP at intervals
- **Webhook/Push**: External system notifies twin of changes

### Browser Plugin: Web App Digital Twins

A browser plugin can extend the DT architecture to web applications. This enables:

1. **URL tokens.** A URL in a task list creates a Resource token. Checking off the task marks it as visited and optionally produces an Information token (notes about the content).

2. **Interaction tracking.** The plugin observes what the user does on a page (forms filled, buttons clicked) and logs it as a sequence of micro-transitions.

3. **SOP extraction.** The system analyzes repeated interaction patterns with the same web app (e.g., monthly hour booking in BCS) and proposes: "You follow this process every month. Want to turn it into a composite transition?"

4. **Progressive automation.** Once the SOP is a sub-PN, individual transitions can be upgraded from manual to automated (via Playwright, browser extension, or MCP tools):

```
Composite: "Book hours in BCS" (🔁 monthly)
  T1: Open BCS → navigate to week view    [Trust Level 3: automatable]
  T2: For each week, enter hours           [Trust Level 0: needs human judgment]
  T3: Add comment for missing hours        [Trust Level 0: needs context]
  T4: Save and submit                      [Trust Level 3: automatable]
```

T1 and T4 are repetitive clicks — the system observed the user always doing the same thing. T2 and T3 require judgment — the system observed different actions each time. The Trust Level follows from observed behavioral variance.

---

## Token Type System

### Type Hierarchy with Mixins

```
Token (abstract)
├── Quantifiable (has amount: add, subtract, transfer)
│   ├── Monetary (BankAccount, Cash, Invoice)
│   └── Temporal (CalendarSlot, TimeBalance)
├── Stateful (has state machine: places + allowed transitions)
│   ├── Document (file-backed, versionable)
│   └── Process (project, workflow)
├── Person (Self-Twin, other people, teams)
│   └── attributes: energy, focus, mental_slots, health, relationships
│   └── composite: attributes projectable as independent tokens
├── Knowledge (epistemic tokens)
│   ├── Information (content, confidence, source)
│   └── Question (content, answer, resolution_status)
└── Resource (external references)
    └── attributes: url, type (article/app/video), visited, notes
```

### Information and Question Tokens

**Information** tokens represent acquired knowledge. They sit in `P_knowledge` and are consumed by transitions that need specific knowledge to proceed.

```yaml
token:
  type: Information
  content: "Finanzamt charged 1,234€ on Feb 15"
  confidence: 0.95
  source: @[[Business Bank Account]]
  acquired_at: 2026-02-15
```

Information tokens make knowledge dependencies explicit and queryable: "What do I still need to find out before I can file my tax return?" becomes a query for missing input tokens.

**Question** tokens are Information tokens in a `pending` state — knowledge that is needed but not yet acquired. They track the resolution process:

```yaml
token:
  type: Question
  content: "Has Finanzamt charged?"
  status: pending | answered
  answer: null | "Yes, 1,234€ on Feb 15"
  confidence: null | 0.95
  resolution_paths:          # via: routes
    - source: @[[Business Bank Account]], estimated_confidence: 0.95
    - source: @perplexity, estimated_confidence: 0.6
```

When answered, a Question token produces an Information token and can trigger downstream transitions.

### Goal Types

The objective function supports three kinds of goals:

| Goal Type | Definition | Example |
|-----------|-----------|---------|
| **Achievement** | Token attribute reaches a target value | Tax return status=filed, car status=purchased |
| **Maintenance** | Token attribute stays in range | Health > 0.7, relationship > 0.8 |
| **Process** | Transition has intrinsic value (doing it IS the goal) | Exercise, meditation, creative work |

This distinction matters: achievement goals benefit from fast completion (discounting), maintenance goals require periodic transitions (exercise, date nights), and process goals make certain transitions valuable regardless of output tokens.

---

## WSJF-Based Task Sorting

The first concrete application of the model. Ships with Phase 1.

### Formula

```
WSJF(task) = CoD(task, now) / duration(task)
```

### Prototype Blocks and `=` Computed Properties

Scoring parameters are defined through **prototype blocks** — normal Holon blocks whose properties serve as both literal defaults and computed expressions. A prototype block is identified by having a `prototype_for` property:

```yaml
Block:
  properties:
    prototype_for: "task"
    default_duration_minutes: 60.0          # literal
    deadline_buffer_days: 3.0               # literal
    deadline_penalty: 200.0                 # literal
    priority_weight: "=switch priority { 3 => 100.0, 2 => 40.0, 1 => 15.0, _ => 1.0 }"
    urgency_weight: "=if days_to_deadline > deadline_buffer_days { 0.0 }
        else if days_to_deadline <= 0.0 { deadline_penalty }
        else { deadline_penalty * (1.0 - days_to_deadline / deadline_buffer_days) }"
    position_weight: "=0.001 * (max_position - position)"
    task_weight: "=priority_weight * (1.0 + urgency_weight) + position_weight"
```

Properties prefixed with `=` are Rhai expressions evaluated at materialization time. Computed properties can reference other properties (literal or computed) by bare name. The system topologically sorts computed properties by dependency and evaluates them in order.

**Prototypal inheritance**: Instance (task) blocks inherit from the prototype. Instance properties override prototype properties:
- Prototype: `priority_weight: "=switch..."`, Instance: `priority_weight: 50.0` → uses 50.0 (literal wins)
- Prototype: `deadline_penalty: 200.0`, Instance: `deadline_penalty: 500.0` → uses 500.0

**Context properties** are injected by the materialization engine at runtime:
- `days_to_deadline` — deadline date minus now (f64::MAX if no deadline)
- `position` — block's position in the list
- `max_position` — total active task count
- `priority` — copied from task block

A built-in `DEFAULT_TASK_PROTOTYPE` Rust const provides the default scoring behavior (identical to the example above), used when no prototype block exists.

### Duration

Read from `duration` property on the block. Defaults to `default_duration_minutes` from the prototype (60 min) when absent.

With the default, all tasks are treated as equal-duration, so WSJF degrades to pure CoD ordering — a safe default that works without any duration estimates.

### List Position as Tiebreaker

When two tasks have identical WSJF scores, the one listed first in the outliner wins. This captures implicit user intent (people naturally list more important things first) at zero friction cost. It also makes the system's behavior predictable — rearranging tasks in the outliner always has an effect, even before adding explicit priorities or deadlines.

### Sort Query

Expressed as PRQL, so it's inspectable, queryable, and replaceable:

```prql
from tasks
filter status != "done"
derive wsjf = task_weight / duration_minutes
sort {-wsjf, list_position}
```

### Configuration as Prototype Block

All scoring parameters are stored as prototype block properties:

- Priority → weight mapping (Rhai `switch` expression)
- Deadline urgency model (Rhai expression referencing `days_to_deadline`, `deadline_buffer_days`, `deadline_penalty`)
- Position tiebreaker weight
- Final `task_weight` formula combining all components

This means:
- Config syncs across devices via CRDT
- Config is editable in the outliner
- Config is versionable (you can see how your scoring evolved)
- The system dogfoods itself
- Custom scoring formulas are just `=`-prefixed Rhai expressions — no code changes needed

### Later: Refinement Loop (Not Phase 1)

When implemented, the system will learn from user behavior:
- User drags a task higher/lower than computed rank → system adjusts that task's base value
- Tasks consistently done first → higher implied value
- Tasks repeatedly postponed → lower value or higher friction (system asks which)

---

## AI in the Petri-Net Model

### Agents as Transition Executors

Agents are generic transition executors. An agent is not a persistent entity with its own goals — it is a stateless executor that receives a briefing and executes transitions.

An agent session consists of tokens:

| Token | Purpose |
|-------|---------|
| **Briefing** | What needs to be done, current context, constraints |
| **Persona** | Communication style, domain expertise to emulate |
| **Capabilities** | Which transitions (tools) this agent session may fire |
| **Context** | Relevant tokens and their current state |

### Autonomy per Transition, Not per Agent

Trust levels (from [VISION_AI.md](VISION_AI.md)) attach to transitions, not to agents:

| Level | Behavior | Example |
|-------|----------|---------|
| 0 — Manual | Human executes | Writing a performance review |
| 1 — Suggest | Agent suggests, human confirms | "Should I schedule this meeting?" |
| 2 — Act & Report | Agent acts, human is notified | Auto-filing emails by project |
| 3 — Act Silently | Agent acts within bounds | Syncing task status bidirectionally |
| 4 — Autonomous | Agent decides what to do | The Watcher alerting on anomalies |

The same agent might fire a Level 3 transition (auto-sync) and a Level 1 transition (suggest rescheduling) in the same session.

### Background Enrichment Agents

A key design principle: **many small, safe tasks** rather than one large autonomous agent. Background agents improve the fidelity of the PN world model without ever taking unsanctioned action:

| Agent Task | Model Size | Risk | Trust Level |
|---|---|---|---|
| Suggest `[[links]]` for plain-text nouns | Tiny (local) | Zero — suggestion only | 1 |
| Propose typed edges between tokens (entity resolution, co-occurrence) | Small (local) | Zero — suggestion only | 1 |
| Classify task as question/delegation/action | Tiny (local) | Zero — suggestion only | 1 |
| Infer likely token type from context | Small (local) | Zero — suggestion only | 1 |
| Suggest dependencies between siblings | Small (local) | Low — suggestion only | 1 |
| Suggest `via:` routes for questions | Small (local) | Low — suggestion only | 1 |
| Answer `?` questions via web search | Medium (API) | Low — produces draft answer | 1–2 |
| Extract SOP from repeated task patterns | Small (local) | Zero — suggestion only | 1 |
| Update Digital Twin attributes from APIs | None (code) | Medium — writes data | 2–3 |

The first six are purely local, purely suggestive, and can run on a phone with a small model. They enrich the PN world model without ever taking action. The user sees a "3 suggestions" badge and can batch-review them.

The safety property: **agents at Trust Level 1 can never change the state of the world. They can only produce suggestions that live in a review queue.** This is fundamentally different from "give AI full computer access" — the PN model constrains what each agent can do by its transition's trust level.

### Confirmation-Driven Edge Creation

The most important enrichment pattern is **proposing typed edges between tokens for human confirmation**. This is a third path between fully manual linking (the user must think of every connection) and fully automatic linking (the system creates edges without oversight):

1. An enrichment agent detects a potential relationship: "Person token Kat appears in 3 task transitions and 2 calendar events related to Project X — but no explicit edge exists between Kat and Project X."
2. The proposal enters a confirmation queue, visible in Orient mode.
3. The user confirms or rejects at System 1 speed (1-2 seconds per decision).
4. Confirmed edges become permanent structure. Rejected proposals are discarded and inform future proposals.

This matters because each confirmed edge increases the density of the token graph without adding new tokens. Denser graphs produce richer context for future proposals, creating a compounding flywheel: more edges → better proposals → higher confirmation rate → more edges. The graph grows smarter without growing larger.

The confirmation moment is not friction to be automated away — it is where cognitive value is created. The user evaluates whether a proposed connection holds within their personal context. This is recognitional judgment (Kahneman's System 1), not effortful deliberation. See [VISION_AI.md](VISION_AI.md) §The Integrator for the full interaction design.

---

## The Outliner as Facade

The Petri net is **not** the user interface. The outliner remains the primary way to interact with Holon. The Petri-net structure is inferred from outliner content:

| Outliner Action | Petri-Net Interpretation |
|----------------|------------------------|
| Create a task block | Create a transition |
| Add a sub-task | Add a transition to a sub-net |
| Add a checklist item (bare noun) | Add a token to parent transition's inputs |
| Mark task done | Fire the transition |
| Prefix with `>` | Add sequential dependency on previous sibling |
| Prefix with `@[[Person]]:` | Create delegation sub-net |
| Prefix with `?` | Create question transition producing Information |
| Set priority | Adjust CoD weight |
| Set due date | Add deadline penalty function |
| Drag task up in list | Increase tiebreaker weight in WSJF |

The Petri-net graph view is an **optional visualization** for users who want to see dependencies, resource conflicts, and bottlenecks explicitly. It is not required for day-to-day use.

### Graduated Complexity

Each level adds value with minimal additional effort:

| Level | What the User Does | What the System Sees | Value |
|-------|-------------------|---------------------|-------|
| 0 | Plain text note | A token | Searchable, syncable |
| 1 | Outline with tasks | Transitions, WSJF from list position | "What should I do next?" |
| 2 | Add `@`, `?`, `>` markers | Delegation tracking, questions, dependencies | Waiting-for list, blocked/ready distinction |
| 3 | Add priorities, deadlines, duration estimates | Full WSJF computation | Intelligent ranking that updates dynamically |
| 4 | Connect external systems (Todoist, JIRA, Calendar) | Digital Twin tokens, MCP transitions | Unified view across all tools |
| 5 | Define objective function weights | Full optimization with simulation | "What if I delegate this project?" |

A user at Level 0 still gets value (notes are searchable, syncable). A user at Level 2 gets automatic waiting-for tracking. A user at Level 4 sees all their work in one WSJF-ranked list regardless of source tool. Each step is opt-in.

---

## Formal Grounding of the Three Modes

The three UX modes from [VISION_UI.md](VISION_UI.md) gain formal definitions:

| Mode | Petri-Net Interpretation |
|------|------------------------|
| **Capture** | Create a new transition or token. Quick, low-detail. The minimum viable input: "something exists that I need to deal with." Typing `@` opens person picker. Typing `?` marks as question. |
| **Orient** | Evaluate the objective function across all open transitions. Show projections (Kanban, pipeline views). Display WIP, slot usage, WSJF ranking, blocked transitions, pending delegations, unanswered questions. |
| **Flow** | The Briefing for one composite transition. All input tokens assembled, context surfaced, distractions hidden. The sub-net for this transition becomes the world. Self DT dynamics track energy/focus. System suggests breaks based on peripheral_awareness threshold. |

---

## Formal Grounding of the Three AI Roles

The AI roles from [VISION_AI.md](VISION_AI.md) gain a computational backbone:

| AI Role | Petri-Net Function |
|---------|-------------------|
| **The Watcher** | Evaluates the objective function continuously. Detects decaying token attributes (health dropping, relationship cooling). Alerts on constraint violations. Identifies blocked transitions and stuck tokens. Monitors `peripheral_awareness` during flow to suggest breaks when high-priority items arrive. |
| **The Integrator** | Navigates the Petri net to assemble Context Bundles. For a given transition, finds all upstream tokens (inputs), downstream consumers (what depends on this), and related transitions (shared resources). Runs background enrichment agents. Routes questions to appropriate `via:` sources. |
| **The Guide** | Tracks mental-slot usage over time. Detects transitions that stay in-progress without progress (Zeigarnik drag). Surfaces Shadow Work via the objective function gradient: "this transition has high gradient but you keep avoiding it." Calibrates Self DT parameters from observed behavior. |

### Simulation Capability

Fork the current marking (all token states), fire hypothetical transitions, evaluate the objective function at future time points. Compare scenarios:

- "What if I finish the tax return this week vs. next month?"
- "What if I buy the used car instead of new?"
- "What if I delegate this project?"

Each scenario is a Petri-net simulation with real numbers from Digital Twins.

---

## Mapping to Existing Architecture

The Petri-net model maps onto Holon's current architecture with minimal structural changes:

| Petri-Net Concept | Current Architecture Equivalent |
|-------------------|-------------------------------|
| Token | Block / Entity (with Digital Twin properties) |
| Place | One per token type. Attribute-based lifecycle phases are projections, not places. |
| Transition | Operation (OperationDescriptor with params + affected fields) |
| Atomic Transition | MCP Tool / OperationProvider method |
| Composite Transition | Sub-net = project block with child blocks as transitions |
| Digital Twin | QueryableCache + SyncProvider |
| Executor (`@` syntax) | Not a separate concept — `@` determines which Person token is in the transition's input/output arcs |
| Briefing | Context Bundle (Flow mode) |
| Information Token | Block with `type: knowledge`, `confidence` attribute |
| Question Token | Block with `type: question`, `resolution_status` attribute |
| Objective Function | Computed from block attributes via PRQL |
| Autonomy Level | Metadata on operations |

### What Already Exists

- **`TaskEntity` trait** (`holon-core/src/traits.rs`): `completed()`, `priority()`, `due_date()` — already token attributes
- **`TaskOperations` trait**: `set_state()`, `set_priority()`, `set_due_date()` — already transitions
- **`QueryableCache<T>`**: already the Digital Twin pattern (local state mirror of external system)
- **`SyncProvider`**: already the twin sync mechanism
- **`OperationProvider`**: already the transition executor
- **`RelationshipRole` enum** (from FSP extensions in VISION_LONG_TERM.md): `BelongsTo`, `ComesFrom`, `LeadsTo`, `Contextual` — directly supports Petri-net edges

### What's New

1. **Task syntax parser** — deterministic parser for `@`, `?`, `>`, `[[links]]`, priorities, deadlines
2. **Verb dictionary** — ~30 German + English verbs → transition type mappings
3. **`@` syntax for executor selection** — determines which Person token is borrowed by a transition (not a new primitive, just syntactic sugar)
4. **Question/Information tokens** — new block types with confidence tracking
5. **Delegation sub-nets** — `@[[Person]]:` materializes as waiting_for pattern
6. **Objective function engine** — evaluates token attributes via PRQL, produces a ranking
7. **Mental slots tracking** — materialized view counting open transitions
8. **Duration property** on blocks — for WSJF computation
9. **Self DT dynamics** — energy, focus, flow_depth, peripheral_awareness with observable signals (window switches, app categories)
10. **Background enrichment agents** — small local LLMs for PN model improvement
11. **Autonomy level** metadata on operations
12. **Token type hierarchy** — extending existing Entity derive macro
13. **Projections** — PRQL/GQL queries that visualize lifecycle phases over the flat net

---

## Phased Integration with Roadmap

How the Petri-net concepts integrate with the existing [VISION.md](VISION.md) roadmap:

| Phase | Existing Goal | Petri-Net Addition |
|-------|--------------|-------------------|
| **1 — Core Outliner** | Usable as LogSeq alternative | Task syntax parser (`@`, `?`, `>`). Verb dictionary. WSJF sorting from priority + due_date + list position. `duration` property. Store CoD config as block. |
| **2 — Todoist** | Prove hybrid sync | Todoist tasks become transitions with DT sync. `@Kat` labels map to `@[[Person]]`. WSJF ranking across Todoist + native tasks. Automatic waiting_for tracking from delegations. |
| **3 — Multi-Integration** | Validate type unification | JIRA integration. Calendar events as time tokens. Question/Information token types. `via:` routing. Type hierarchy with shared traits. Unified WSJF across all sources. |
| **4 — AI Foundation** | Infrastructure for AI | Background enrichment agents (local LLM). Self DT with energy/focus dynamics. Objective function engine. Mental slots as materialized view. Confidence tracking on Information tokens. |
| **5 — AI Features** | Three AI services | Simulation engine (fork marking, fire hypothetical transitions, compare). Agent briefing from PN context. Watcher uses objective function + peripheral_awareness. SOP extraction from repeated patterns. |
| **6 — Flow** | Users achieve flow states | Full Self DT dynamics with emergent work rhythms. Context Bundles = transition briefings. Orient mode = objective function dashboard. Browser plugin for web app DTs. Graduated complexity (Levels 0–5) fully realized. |

---

## Design Decisions

### Petri Net: Implicit, Not In-Your-Face

The Petri net runs in the background. Users interact with an outliner. The net structure is inferred from block types, syntax markers, and status attributes. An optional graph visualization is available for power users in later phases. The words "Petri Net" never appear in the user interface.

### Structural Primacy: AI-Optional by Design

The system is fully functional with zero AI. The syntax parser, verb dictionary, WSJF engine, Petri Net materialization, and dependency tracking are all deterministic and always run. AI enrichment is a separate, optional pipeline stage that produces suggestions — never auto-applied changes.

This follows the **substitution test**: if we swap the AI model (replace one LLM with another), the system continues to function with the same knowledge base. If we remove the structural layer (Turso cache, Loro documents, entity graph, Petri Net), no AI model can reconstruct it. The structure is irreplaceable; the model is not. When evaluating new features, ask: "Is this a structural investment or a model investment?" Prefer structural investments — they compound over time, work offline, and survive model upgrades.

### Safe Defaults for References

A bare `[[reference]]` in task text is treated as **context** (tag), not a dependency. This prevents false blocking — a task is never grayed out because the system misinterpreted a reference. Input arcs are only created through explicit syntax (`@[[Person]]:`, verb-object grammar) or user-confirmed AI suggestions. The cost of a missed dependency (task appears when it shouldn't) is much lower than the cost of a false dependency (task hidden when it should be visible).

### Integration-First, Not Replacement

Holon integrates deeply with existing tools (Todoist, JIRA, Calendar) rather than replacing them. Users get immediate value (WSJF ranking, waiting_for tracking) without migrating. Native task creation is optional and additive. The syntax maps cleanly to Todoist's label system (`@Kat` → `@[[Kat]]`), ensuring mobile capture remains fast even before a native Holon mobile app exists.

### Token Types: Same IR for Code and User-Defined

The `#[operations_trait]` macro and `Entity` derive macro generate implementations from Rust code. User-defined token types will compile from a schema format (YAML or DSL) to the same intermediate representation. This means the macro system should target an IR early, even before user-defined types are exposed.

### CoD: Real Value, Not Artificial Motivation

Cost of Delay is derived from real token attributes (money × time-value, forgone utility per day, penalty functions near deadlines). It is never an artificial number assigned to motivate action. If a task has no real cost of delay, its CoD is low — and that's correct.

### Objective Function: Data, Not Code

The objective function weights, priority mappings, and custom scoring formulas are stored as prototype block properties with `=`-prefixed Rhai expressions. They are editable, syncable, and versionable. The scoring formulas can be inspected and replaced by editing the prototype block — no Rust code changes needed. A default task prototype is provided as a Rust const for backward compatibility.

### Flat Net with Projections, Not Hierarchical Places

The net uses one place per token type. Attribute-based lifecycle phases (draft → reviewed → published) are **projections** — queryable views over the flat net, not structural elements. This avoids the stranded-token problem and supports arbitrarily overlapping attribute dimensions without combinatorial explosion. Projections are PRQL/GQL queries, stored as Holon blocks, renderable as Kanban boards or SOPs.

### Children Are Either All Tokens or All Transitions

A task's children should be uniformly tokens (checklist) or transitions (sub-net), not mixed. This keeps the PN materialization unambiguous and matches natural usage — packing lists don't contain sub-projects, and project plans don't contain bare nouns. When mixed intent arises, the solution is restructuring under a common parent.

### Refinement: Deferred

The feedback loop (user reorders → system adjusts weights) is valuable but complex. Phase 1 ships with static weights. Adaptive learning comes later, when there's enough usage data to learn from.

---

## Related Documents

- [VISION.md](VISION.md) — Technical vision and phased roadmap
- [VISION_LONG_TERM.md](VISION_LONG_TERM.md) — Philosophical foundation (Integral Theory, FSP extensions)
- [VISION_AI.md](VISION_AI.md) — AI roles, Trust Ladder, Shadow Work
- [VISION_UI.md](VISION_UI.md) — Three modes, calm technology, design system
- [ARCHITECTURE.md](ARCHITECTURE.md) — Current technical architecture
- [BUSINESS_ANALYSIS.md](BUSINESS_ANALYSIS.md) — Strategic analysis, competitive positioning, go-to-market
- [SWOT.md](SWOT.md) — SWOT matrix
- [digital-twins-petri-netze-conversation.md](digital-twins-petri-netze-conversation.md) — Original design conversation (full transcript)
