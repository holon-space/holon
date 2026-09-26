---
id: 2026-09-30-a-template-with-a-task-fails-mid-instantiation
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  instantiate_template on a template that holds a task block failed at that
  block's create, after the creates before it had run, because the plan copied
  task_state inside the `properties` bag, which the engine refuses.
---

## Bug
Found by the Inc 6 round-13 lane while adding a ring check to
`instantiate_template`: any template child carrying a task keyword made the
instantiation fail with `create: refusing property key "task_state" … via
properties`, after the creates before it had already run.
`lane-logs/inc6r13-red-template.log`.

## Root cause
`plan_instantiation` (`crates/holon-api/src/template_instantiation.rs`) copied
the node's whole property bag, `task_state` and `task_state_category`
included, into the create's `properties`. The engine refuses a typed field in
that bag. Even with the keyword accepted, the copied category came from the
template's document, not the target's.

## Missing piece
The template tests used only templates without tasks, and no keystone
transition instantiates a template.

## Remedy
The plan carries `task_state` as its own create field and drops the category,
so the engine classifies it by the target document's ring. The engine judges
every template keyword against that ring before the first create
(`refuse_template_keywords`). Red/green:
`a_template_keyword_the_target_ring_lacks_is_refused_before_any_create` and
`an_instance_task_takes_the_target_documents_category` in
`crates/holon/tests/cycle_task_state_vocabulary.rs`
(`lane-logs/inc6r13-red-template.log`, `lane-logs/inc6r13-red-template-judge.log`).
