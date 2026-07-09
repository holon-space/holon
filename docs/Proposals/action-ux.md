# Action UX — MVP and advanced (companion to ADR 0024)

**Status:** Direction agreed with Martin 2026-07-09 (session "holon: Actions vs
Petri-Net"); decided items marked DECIDED, forks marked OPEN.
**Basis:** [ADR 0024](../adr/0024-unified-action-execution.md). Everything below
is a consequence of its substrate choices: rules/tokens/history are blocks, so
the UX is **render profiles over vault data, not a new app**. The PKM is the
automation IDE.

## Personas

1. **Author** — writes rules. Human first; increasingly an *agent* (agents
   propose rules as advice; accepting materializes the rule block).
2. **Observer** — the same human later: "what did my automations do, and why is
   this block here?" Trust is a UX deliverable; it decides whether rule #2 ever
   gets enabled.
3. **Deliberation consumer** — authors nothing; receives plans/suggestions via
   the advice channel and accepts or rejects.

## North star: a Kanban board is a Petri net render

Columns = places, cards = tokens, dragging a card = `move_block` = manually
firing a transition. Users operate nets long before they author them; the board
is the on-ramp. Authoring emerges later by attaching rules to places users
already have.

---

## MVP (one-transition-net era; ADR 0024 Phases 1–3)

### Authoring: text-first `when/then` blocks

One rule = one source block, same YAML-ish dialect family as
`holon_advice_rule_yaml` (one authoring model, effect kinds `advise` |
`operate`):

```
* Daily journal
#+begin_src holon_rule
when: today and not journal(date = today)
then: block.create
  parent: journals
  name: "{today}"
#+end_src
```

- **DECIDED — no `unless` keyword.** A separate `unless` forces the author to
  classify complicated predicates into when-vs-unless arbitrarily. One `when`
  with full boolean composition (`and` / `or` / `not` — the Pattern AST has
  `Not` anyway); negation is spelled inside the predicate.
- **DECIDED — text-first, not form-first.** Matches org DNA and
  agent-authorability; the novice path is "agents draft the text, humans read
  it", not a wizard. Discovery via the block-type / slash menu ("Rule…") with
  3–4 seeded templates; the shipped Journals rule doubles as the tutorial.

### Rendering: the rule card (program marking as a feature)

A rule block renders as a collapsed **rule card**: name, enabled toggle,
`last fired: …`, and — fail-loud made visible — a red error state showing the
actual guard-compile or effect error. Rules become the *most* legible blocks on
the page (inverting the current bug where they render as broken query results).

### Trust primitives (both from day one)

- **Provenance badge:** every auto-created block carries a subtle ⚙ affordance
  → "created by *Daily journal*, today 00:03" → click-through to the rule card.
  This is ADR 0024 P8 (`fired-by`) surfaced as UI.
  OPEN — loudness: always-badge vs badge-until-acknowledged vs hover-only.
  Lean: badge-until-acknowledged for new rules, hover-only once trusted (trust
  decays the ceremony).
- **Dry-run before enable:** enabling a new rule first shows "this would fire
  3× right now → [list]"; confirm or cancel. This is the in-memory evaluator /
  simulator doing its first product job, and it converts automation from scary
  to boring.

### Auditing: the Automations page is a query

Grouped-by-rule-and-day view over provenance-stamped effects. No new storage or
view machinery — an ordinary (hence user-customizable) page. Rows should be
designed so a later per-firing **undo** (inverse ops — a kept-warm invariant)
can attach to each row; undo itself is post-MVP (dry-run prevents most regret).

**MVP is exactly:** rule card + when/then sugar + provenance badge + dry-run +
Automations page. No net editor, no graph view, no board requirement.

---

## Advanced (ADR 0024 Phases 4–5)

### Nets you use before you author

Board render-profile over a subtree → user attaches a rule to a place ("when a
card enters *Done*, stamp completed-date") without ever seeing "Petri net".
Multi-transition nets are authored as outlines (net = subtree; places =
headings; transitions = rule blocks); a graph view is just another render
profile of the same subtree.

### Enabled-transition surface (manual transitions in context)

The net computes what you *can* do; the UI offers it where you're looking
("3 actions available here").

**DECIDED — presentation ladder** (same enabled-transition action set, three
presentations — render profiles again):

1. **Default: in-context action bar** on click/focus at the block. Rationale:
   transitions are *named, user-defined* actions, and horizontal text is the
   highest-bandwidth label format; works on touch; near-zero pointer travel.
2. **Pointer/pen upgrade: marking-menu mode** (right-click / long-press →
   radial; flick = eyes-free fire). Radial menus' concrete drawbacks — label
   geometry, ~8-slot angular ceiling, occlusion under finger, no hover on
   touch, zero toolkit support — mostly don't bite here: enabled sets are
   small (1–5), GPUI is fully custom, and a PKM is exactly the expert-dense
   high-repetition niche where marking menus' measured ~3× eyes-free speedup
   (Kurtenbach/Buxton) pays. They failed in *general* software because it
   optimizes first-contact discoverability over 1000th-use speed — a trade
   that goes the other way for us.
3. **Keyboard-first: which-key-style popup** on a leader key — for org-people,
   likely the most-used of the three.

### External effects: the lease made visible

Once-effects (ADR 0024 P4) show device ownership on the rule card: "Emails are
sent by *MacBook* (lease expires in 6 h)" + "take over on this device". The
distributed-systems honesty becomes a feature — users see which machine acts.

### Deliberation rides the advice channel

- "Plan my day" → an advice card stack: proposed ordering, one-line *why* per
  item (guard/score made legible), accept-all / accept-one / dismiss.
  **Accept = fire the real transitions** (P2: committing a plan is firing);
  the planner never gets its own modal world.
- What-if = same surface + **ghost-state render mode**: the simulator's forked
  world drawn over the normal UI in visually distinct "hypothetical" styling,
  with a time scrubber (this afternoon → Friday).
- The AlphaGo-grade planner changes suggestion *quality*, never the
  interaction contract ("plans arrive as advice; nothing happens until you
  accept") — intelligence scales under a stable UX.

### Agents as authors — the flywheel

Agent notices a repeated manual pattern → proposes a rule *as an advice card* →
accepting materializes the rule block → which gets dry-run like any rule.
Composed entirely from existing pieces (advice + rule card + dry-run); zero new
UI. This is where "almost arbitrarily intelligent" meets "user stays
sovereign".

---

## Open forks (queued for a design pass)

1. Provenance badge loudness (lean: badge-until-acknowledged → hover-only).
2. Undo scope: per-firing / per-run / day-level (MVP: none; journal rows must
   leave room for per-firing).
3. Action bar placement detail: inline in the outline vs side rail with
   per-block affordance (lean: prototype the side rail).
4. Ghost-state visual language (needs design exploration; candidate for
   `design_gallery` mockups).

## Prototyping venue

`frontends/gpui/examples/design_gallery.rs` + `holon-frontend`'s
`widget_gallery` — the standalone render-pipeline demo on hard-coded data — is
the venue for mocking these up (rule card, provenance badge, Automations page,
action bar, dry-run dialog) as a new gallery mode before any backend work.
