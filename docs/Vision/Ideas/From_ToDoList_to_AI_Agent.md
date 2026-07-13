# From the To-Do List to the AI Agent — Interface Studies

Source: https://www.youtube.com/watch?v=OuEKdD_1F8s (Interface Studies, ~16 min)

> Note: this is a paraphrased, structured summary of the video (not a verbatim
> transcript — reproducing the full transcript would be a copyright issue).
> Timestamps are approximate anchors into the video.

## Core thesis

A productivity app is not just "the same list, on a screen instead of paper."
It's a **bundle of trades**: at every step it gives up something the old way
(paper) had, in exchange for something the old way lacked. You can read almost
any productivity app by asking *which trades it made*. Most apps make the same
trades; a few refuse them. The video walks through four such trades, then
argues that AI agents are the newest and deepest version of the same pattern.

## Why there has to be a trade at all (0:00–3:15)

- Sellen & Harper, *The Myth of the Paperless Office* (2001): measured that
  adopting email actually *increased* paper/printing in the organizations they
  studied — computers didn't replace paper, because paper isn't "a worse
  screen."
- **Affordance** (Gibson, 1960s; brought into design by Don Norman): what an
  object lets you do. A handle affords pulling; a flat surface affords putting
  things down. Paper affords being spread across a desk, written on at any
  angle, held in two hands, taken in at a glance — no screen reproduces all of
  that.
- So when work moves off paper, it can't carry every affordance across. It
  keeps the ones that turn into data, and trades the rest away. The trade is
  unavoidable; the only question is *which* trade a given app makes.

## Trade 1 — Structure vs. freedom (3:16–4:35)

- Underneath, almost every task/notes app is a list of records with fields.
- Apps that are open about it (Notion) hand you a database directly: every
  thought is a row, and a row has columns whether you fill them or not. It
  doesn't force classification, but the structure "wants feeding" — writing
  something down becomes filing it.
- Capture-first tools (Apple Notes, Drafts, Sticky Notes) sit at the other
  end: write now, sort later or never.
- **Trade: order in exchange for the freedom to not yet know what a thing
  is.** A choice, not a law — Notion takes order, capture-first tools take
  freedom; same trade, opposite signs.

## Trade 2 — Place (4:35–6:56)

- "Views" (list/board/calendar, sort/filter) look like pure progress, but hide
  a cost: on a desk, a thing has a location whether you want it or not, and
  that location *means something*. In a database the default is that nothing
  sits still — change the sort and the item that was near the top reflows
  somewhere else.
- Supporting research cited: Thomas Malone (1983) — desk "mess" wasn't mess,
  where a thing sat told you how urgent it was. David Kirsh — arranging
  things in space is part of thinking, not a leftover of it. Bergman &
  Whittaker — people re-finding files often prefer navigating back to a
  remembered place over searching; search answers "what was it called," not
  "where did I leave it."
- Software didn't abolish place, it made it *optional* — you can build a
  workspace that becomes a place (a dashboard you know by heart), but you have
  to build and maintain it and keep resisting the urge to rearrange.
- **Trade: place that used to arrive for free, in exchange for place you now
  have to keep making.** The spreadsheet is called out as the clearest
  counter-example — nobody thinks of it as a "productivity app," but B7 is
  always B7, and the grid holds still long enough to learn.

## Trade 3 — Guilt (6:57–7:35)

- A paper list got thrown away; rewriting tomorrow's list was a quiet daily
  act of deciding what still mattered.
- Apps archive instead of discard — nothing is ever quite gone, which sounds
  like a gift and behaves like a debt. The backlog that leaves you feeling
  permanently behind isn't laziness — it's the natural shape of a container
  with no bottom.
- **Trade: total recall in exchange for the edge that used to force you to
  choose.**

## Trade 4 — Ownership (7:36–9:00)

- Cloud apps: the company holds the real copy, you hold a view of it. That
  buys real things — sync, collaboration, backups.
- Ink & Switch's local-first research: when data lives on someone else's
  servers, you become a *borrower* of your own work — if the service shuts
  down or locks you out, the work can go with it.
- Obsidian ("file over app"): every app is eventually obsolete, so what you
  want to last should be files you control, in a format anyone can read.
  Plain text on your own disk.
- **Trade: Notion trades ownership for collaboration/polish; Obsidian trades
  collaboration/polish for ownership.** Same trade, opposite ends, neither
  simply right.

## Why most apps land on the same side (9:00–11:03)

- Partly history: the database/cloud/bottomless-store architecture goes back
  to Borland Sidekick (1984), Lotus Agenda. David Allen's *Getting Things
  Done* (2001) gave the philosophy — "capture everything, get it out of your
  head into a system you trust" — that made people want the machinery Allen
  didn't invent.
- Partly the market: Will Manidis's concept of **"tool-shaped objects"** —
  things built like tools that mostly produce the *feeling* of work, because
  the market for feeling productive is far larger than the market for being
  productive. A tool sold to that market rewards the part of you that enjoys
  arranging the system over the part that wanted the task done. The
  local-first/plaintext crowd sells to a smaller market that wants control
  and longevity instead, and pays for it in convenience.

## The "people are mostly fine" objection (11:03–12:00)

- An affordance was never a rule — a missing one only costs you if you were
  doing the thing it supported. Someone with 5 chores and someone with 3
  projects and 200 notes are not doing the same work.
- Herbert Simon's **satisfice**: almost nobody chooses a tool by auditing its
  trade-offs — we take the first option that's good enough and stop, with
  ties broken by habit / what the team uses / what a friend swears by.
- This is exactly why the "refusers" (sticky-note users, plaintext-folder
  writers, local-only Obsidian users) matter: they're proof the trade was a
  *choice*, not a law — though even Obsidian users can slide back into
  tending an elaborately plugin-configured vault instead of using it.

## Trade 5 — The agent, the deepest version yet (12:00–end)

- Pitch: you can capture if you want, but don't have to — the agent captures.
  You won't triage — it triages. You won't even do the task — it drafts the
  email, does the reading, runs the steps, and tells you when it's done.
- This argument already happened once: **Shneiderman vs. Maes, "Direct
  Manipulation vs. Interface Agents" (1997)**. Then, an "agent" meant software
  that learned your preferences and took over a chore (filtering email,
  recommending music, booking meetings) — Clippy was its face. Shneiderman,
  the lifelong advocate for direct manipulation (visible things you act on
  and watch respond), was wary: he worried about predictability, control, and
  being able to understand what the software had actually done while working
  unseen. His argument: the bill comes due as a loss of visibility and
  control. In 1997 that was mostly hypothetical — now it's real.
- An AI agent takes a vague instruction in plain words and acts across your
  applications while you're not watching, then returns a result you can't
  glance at the way you glance at a desk, and can't take hold of and move.
  You hand over a sentence and trust what comes back.
- This is the far end of everything traced so far: the desk had visible
  objects, direct handling, a place, your own hands on the work. The app took
  the place; the cloud took the ownership; the agent takes the handling *and*
  the visibility. Same axis as "file over app," scaled up — the earlier
  question was *who holds your files*; this one is *who does your thinking*.
- It would be easy — and unfair — to call this a mistake: **Maes was right
  that not every task deserves your attention**, and handing one off can be
  the whole point. Eric Horvitz (1999) argued for a middle road: **mixed-
  initiative systems**, where person and machine take turns instead of one
  replacing the other — built deep into the agent itself, not bolted on.
- Closing framing: direct manipulation charges you attention in return for
  control; the agent takes the attention away and charges you visibility
  instead. Which bill you'd rather pay depends on the work in front of you.
  For 40 years software has traded place for retrieval, ownership for
  convenience, direct handling for delegation — mostly by default, with a
  stubborn few refusing, which is the clearest proof these were trades and
  not laws. The agent is the latest trade on offer; what it really asks isn't
  where your notes live, but **who does the thinking, and whether that's a
  trade worth making — for everything.**

## Cross-reference

See the take-aways already captured in [`../Ideas.md`](../Ideas.md) under
"From the To-Do List to the AI Agent" for how this maps onto Holon's own
design trades (blocks vs. free placement, a central arrival surface, etc.).
