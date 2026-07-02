<!-- Authoritative design (2026-06-14). SUPERSEDES docs/Testing/PbtSlicing.md
     and docs/Testing/PbtSlicing_Trivialization_Handoff.md. -->

# PBT Composition — making an arbitrary system slice trivial to test

**Status:** design accepted; PoC + sizing spike landed (§7); Step 0 ✅, Step 1 🟡 (full-e2e parity pending), Step 2 🟢 first composed memory slice landed via path B (§8 Step 2, 2026-06-15).
**Supersedes:** `PbtSlicing.md` (drifted from code — §4/§12/§13 describe `MemBlockStore`/`MemEditorMirror` that don't exist) and `PbtSlicing_Trivialization_Handoff.md` (its Move-B `unimplemented!()` macro is rejected here — see §3).
**Evidence in tree:** `crates/holon-pbt-core/src/composition.rs` (compiling PoC, 5 green tests).

---

## 1. Goal

Testing *any* slice of the system should be a matter of **listing components**:

```rust
// "GPUI with the memory backend" — and that is the whole per-slice surface.
let config = Config::new().with(MemoryBackend::new()).with(GpuiFrontend::test());
```

The framework then **derives** which generators/transitions and which invariants apply to that slice, runs them, and discloses what it skipped. No god-type, no per-slice invariant list, no per-combination boilerplate.

Today this holds only for *narrow* hand-written slices (`tests/editor_pure_pbt.rs`). A slice that wants the **wide invariant registry** (the ~35 cross-subsystem bodies the big E2E slices share) pays a ~2000-line capability tax (`pbt/sut_capabilities.rs`). This design removes that tax.

---

## 2. The model (sets & relations)

Think in capabilities, not Rust types.

- **Capability** `c` — a named bundle of operations. These already exist in the code as the `Sut*` traits (what a SUT does/observes) and `Ref*` traits (what the reference model exposes). Let `C` be the full set.
- **Component** `k` (a sub-SUT or sub-Ref) **provides** `prov(k) ⊆ C` and may **require** `req(k) ⊆ C` (e.g. the Turso projection requires a backend's CDC stream).
- **Configuration** — a chosen set of components. Provided set `P = ⋃ prov(k)`. **Valid** iff `req(k) ⊆ P` for all `k`. An invalid config (e.g. "Turso without a backend") must **fail loud at construction**, not silently drop invariants. *This is enforced NOT by a declarative `CapProvider::req()` but by the **fail-loud construction path** itself: `CapMap::insert` panics on duplicate cap registration (naming the cap), `CapMap::replace` panics if the overridden cap was never registered, and the handful of composed builders carry fail-loud asserts for their own wiring preconditions (`compose_sut`'s `!has_actor(UI)`, `ComponentSet`'s `Actor::UI ⟹ ViewModel`). C-1 audit (2026-07-02) found no third hand-defended guard site, so `req()` was NOT added — builder asserts + fail-loud `insert`/`replace` ARE the validity mechanism. See §4.2.*
- **Consumer** — an invariant `i` or a transition `g` declares a **need-set** on two axes: `need_sut` and `need_ref`. (An invariant compares a SUT projection against a Ref projection; a transition needs ref-mutation caps to model the change and sut-apply caps to execute it.)

**Selection rule (positive + negative containment):**

```
i runs  ⟺  need_sut⁺(i) ⊆ P_sut  ∧  need_sut⁻(i) ∩ P_sut = ∅  ∧  need_ref⁺(i) ⊆ P_ref
```

The **negative** half (`need_sut⁻`) is the one new primitive vs today's runtime selection. It lets a *degraded-mode* invariant run only when a capability is **absent** (see §5).

Two asymmetries that matter:

- **The SUT composes as a union of independent components** (backend ⊕ frontend ⊕ projection).
- **The Ref *core* does not.** `ReferenceState` is one *coupled* model (a `SplitBlock` mutates block-state *and* editor cursor atomically). On the Ref side, "sub-Ref" means a *capability-view/projection of one coherent model*, not a union of parts. (The transitions are already written generic over `Ref` capability traits — `split_block_apply_to_ref<R: RefBlockTreeMut + RefFocusMut + …>` — so Ref-side composition for *generators* already works.) When two subsystems write the *same* datum and the outcome is a merge ("neither X nor Y"), the coupled core still owns it — it may compute the merged value via a **not-under-test merge engine (Loro) driven from intent**, never by storing two copies. See §5.4. **Caveat — "coupled core" ≠ "closed struct":** the core not composing constrains only *shared* data; the Ref as a whole may still be an **open registry of per-module *private* extensions** over that core, which is what makes new-subsystem contribution (even via git submodule) possible. See §5.5.

---

## 3. Why the status quo is hard, and the design theorem

The pain is **not** selection — `PbtSuiteSpec::select` already does `min_sut ⊆ subsystems` at runtime. The pain is the **Rust typing of composition + dispatch**: today a single statically-typed `Vec<Box<dyn DynInvariant<R, S>>>` names *every* body, so the one `S` must satisfy the **union** of every body's capability bound. That union is the ~2000-line tax, and "faking" the missing caps with `unimplemented!()` (the handoff's Move B) just relocates it and trades a compile error for a runtime panic — against this repo's "make illegal states unrepresentable / fail loud at the boundary" rules.

**Theorem.** Once you stop naming all bodies in one statically-typed list, you are forced into exactly one of two worlds:

- **ζ — compile-time filter, static provider.** Bodies stay `fn check<S: need(i)>(&S)`; a macro filters *which bodies exist* per SUT by the SUT's declared cap-set. No faking, but every configuration needs its own combined SUT type, and arbitrary composition is not free.
- **γ — runtime filter, dynamic provider (typemap).** The SUT is a `CapMap: cap → Arc<dyn Cap>`; bodies look up the caps they declared; selection runs *before* dispatch so every lookup is a proven containment. Composition is `.with(component)` — **no per-combination type at all**, even E2E.

The status-quo "static provider + runtime list" is the worst of both: it's what *creates* the union requirement.

**Decision: γ.** It is the only option where an *arbitrary* slice is genuinely "list the components." See §4. (ζ remains a fallback if the async-trait object-safety work in §4.4 ever proves unworkable — it didn't; see §7.)

---

## 4. Chosen design: γ (capability typemap)

### 4.1 `CapMap` — the composed SUT/Ref

A map keyed by each capability's trait-object `TypeId`, storing the provider as `Arc<dyn Cap>` (one component's `Arc` backs every cap it provides via unsizing coercion). `get`/`expect`/`cap_set` round-trip trait objects through `Any`. `expect` panics loud with the present-set on a *selected-but-absent* cap — an assertion of an already-proven fact (selection ran first), never an `unimplemented!()` stand-in.

The single "implements every cap by lookup" surface lives **once**, generically, on `CapMap`/the caching wrapper — it does **not** recur per new SUT. That is the structural difference from the rejected macro.

### 4.2 Components & `Config`

```rust
trait CapProvider { fn register(self: Arc<Self>, caps: &mut CapMap); }

struct Config { /* … */ }
impl Config { fn with<P: CapProvider>(self, c: P) -> Self; fn build(self) -> CapMap; }
```

A configuration is just a component list. `build` returns the assembled `CapMap`; validity is not a
separate declarative pass but is enforced *along the construction path itself* (see the resolution box).

> **Resolution (2026-07-02, C-1 verdict = "bless the builder asserts"): there is deliberately NO
> `CapProvider::req()`.** `Config::build` returns `self.caps`; a declarative `req ⊆ P` pass was
> considered and rejected. The validity mechanism is instead the **fail-loud construction path**:
> 1. **`CapMap::insert` panics on duplicate cap registration** (C-2, 2026-07-02) — one-provider-per-cap
>    is now enforced by the type, naming the offending cap. Silent shadowing is unrepresentable.
> 2. **`CapMap::replace`** is the honest counterpart for a *deliberate* override (a builder registers a
>    component's default provider, then swaps in a specialised one under the same cap `TypeId`); it
>    panics if the cap was never registered first, so a stale precedence assumption also fails loud.
> 3. **Builder-side asserts** guard the remaining wiring preconditions that are not cap-duplication
>    (`compose_sut`'s `!has_actor(UI)` for the headless/windowed thread-affinity split; `ComponentSet`'s
>    `Actor::UI ⟹ ViewModel`).
>
> The C-1 audit (2026-07-02) searched every composed builder for a *third* hand-defended
> duplicate-guard site beyond the two known ones (`sql_loro_wide`, `overlay_windowed_caps`) and found
> none — so no general `req()` abstraction is warranted. `overlay_windowed_caps`' hand-rolled
> `no-prior-SutDriver` assert was **deleted** (redundant: its `register` calls now trip `insert`'s
> panic). `sql_loro_wide` keeps its selective single-cap insert (a precedence composition that avoids
> the collision, not a guard dodging the panic). This closes §2's "the current system has no such
> check" — the check exists, distributed across `insert`/`replace`/builder asserts. Tracked in §11 (C-1, C-2).

### 4.3 Single-sourced invariants (`cap_invariant!`)

One declaration drives **both** the selection metadata (`needs()`) **and** the in-body cap lookups, so the declared need-set and the actual reads cannot drift:

```rust
cap_invariant! {
    name: InvNoOrphanBlocks, id: "inv-no-orphan-blocks", mode: Strict,
    sut: { backend: dyn SutBackend },        // → needs() AND `let backend = sut.expect::<dyn SutBackend>()`
    sut_absent: [],                          // negative needs (degraded twins)
    ref: { tree: dyn RefBlockTree },
    check: { for id in backend.live_block_ids().await { /* … uses backend, tree … */ } }
}
```

### 4.4 Read caps vs write caps (proven by the §7 spike)

The real `Sut*` traits use native `async fn` (**not** object-safe → `dyn SutBackend` illegal) and some take `&mut self`. The typemap forces `dyn`. Resolution, confirmed compiling:

- **Read caps** (snapshots that invariants read — `live_block_snapshot`, `cdc_in_flight`, …) are already `&self`. Make them object-safe with **`#[async_trait(?Send)]`** (`?Send` because `E2ESut` futures hold non-Send gpui/Rc state) and put them in the `CapMap`.
- **Write / drain / quiesce caps** (`drain_cdc`, `quiesce`, `apply_split_block` — all `&mut self`) **stay `&mut self` on the concrete SUT** and run only in `apply`, which *owns* the SUT. They never enter the Arc-shared map, so they need **no** `&mut → &self` conversion.

So the migration is dominated by "add `#[async_trait]` to the read-cap traits" — not a sweeping signature rewrite. (The PoC additionally proves a *write* cap can live in the map via `&self` + interior mutability, should a slice ever want apply-free mutation — but that is not required for the standard design.)

### 4.5 The runner & caching

`run_selected(registry, sut: &CapMap, ref_: &CapMap)` (async) is the extracted, SUT-agnostic core (the handoff's "Move A"): select by §2's predicate, dispatch survivors, disclose `deselected`. The per-tick memoisation that today lives on `CachingProxy<S>` becomes one generic lookup-and-memoise wrapper over `CapMap` — written once.

`StateMachineTest` glue is unchanged in shape: `apply` mutates the concrete SUT through write caps; the **sync** `check_invariants` does `runtime.block_on(run_selected(...))` (exactly as `E2ESut` does today).

---

## 5. The Ref side

### 5.1 Omnipotent core + projection caps

- **State core: one omnipotent, coupled model.** Holds every dimension (blocks, editor, focus, peers, watches, files). Unexercised dimensions rest at identity — *don't generate the transition and the capability stays hidden* (your peer/`AddPeer` intuition, which holds exactly for additive/orthogonal caps).
- **Transitions: cap-generic, backend-agnostic, mutate the core.** Generated iff their `Ref*` cap-bounds ⊆ config. Already true in code.
- **Projections: caps over the core.** Invariants read `ref_.expect::<dyn RefBlockTree>()` etc.

### 5.2 Config-variance audit result: zero in-body variant projections

Audited all 40 invariant bodies (see git history / the analysis that produced this doc). **No body computes a Ref-expected value that branches on the capability set.** Config-variance is handled entirely by (a) selection and (b) SUT-internal coherence checks that self-skip. Specifically:

- **Degraded query rendering** (GPUI shows a query block's *source* when no query engine is wired — real prod path: `loro_ui_watcher.rs` `source_editor_expr` via `holon-app/no_turso.rs`) is **not** a branching projection. The Ref doesn't model query results at all (`viewmodel_decompiled_rows_match_query` is SUT-internal: `root_content_comparison` returns *both* sides). It decomposes into **complementary invariants selected by the query cap's presence/absence** — which is exactly what negative selection (`sut_absent`) provides.
- **Windowing** (blur/commit bimodality, `EngineFocus`) produces **no** Ref-value-variance: `focus_matches_ref`/`editor_text_matches_ref`/`editor_caret_matches_ref` compute the expected value as an unconditional `ref_.*` read. The bimodality lives in the transition (`commit_active_editor_if_dirty`, dirty-gated) and SUT-side settle, plus selection.

**Soundness rule:** whenever the SUT degrades on capability-absence, the Ref must degrade through the *same* `CapSet`, or you manufacture a false divergence. γ gives this for free (one `Config` drives both) — provided degraded behaviour is routed through capabilities, not hard-coded on either side.

### 5.3 Validity: hard vs soft (degrading) dependency edges

- **Hard req** — component is nonsense without the cap (Turso projection ⊸ a CDC source). Missing → config invalid, fail loud at `Config::build`.
- **Soft / degrading req** — component functions in a *disclosed* degraded mode (GPUI ⊸ query result → shows source; outliner tree still works). Missing → both SUT and Ref take the degraded projection; the harness should assert the degraded banner shows so degradation can't masquerade as full mode.

### 5.4 Interacting subsystems: when the expected value is a *merge* (2026-06-26)

§5.1 says the Ref is one coupled core whose sub-Refs are projections, "never a union of parts." That rule answers *code structure*; it does **not** by itself say how the core computes the result when two subsystems write the **same datum** and the outcome is neither subsystem's value — the canonical case being the same block edited in an org file and in the UI, where the result is **Loro's CRDT merge**. This section is the standing answer, and it refines §5.1/§9 without reversing them.

**First, most "interactions" aren't merges — classify the pair of writes:**

| Pair | Reference treatment | Oracle archetype |
|---|---|---|
| **Orthogonal / commuting** (different data, or disjoint *footprints*) | apply both in any order → exact, *no merge engine*, algorithm-independent | construction (10/15) |
| **Sequential same-target** (settle between transitions) | last-writer / fold in defined order — still a *function* the core predicts exactly | construction (10) |
| **Concurrent same-target, overlapping footprint** (true conflict) | reuse the merge engine *or* drop to convergence | 20 or 25 |

The crucial framing: the "neither X nor Y" outcome arises **only under genuine concurrency** (divergent histories merged — i.e. the multi-peer regime). Under our settle-between-transitions harness (quiescence is the harness's job, §8 of `PbtSlicing.md`), a UI edit lands in Loro *before* the next org edit reads it, so there is no conflict to model. **The merge problem is the multi-peer regime's problem**, and that regime is already a distinct tier with a property oracle — `subsystem_convergence_pbt` / `loro_sync_controller_pbt` (S1–S3/C1–C3 convergence laws), deliberately kept separate because rewriting Loro's CRDT semantics into the block model "is not even semantically possible" (`PbtSlicing.md` §12).

**The overlapping-conflict residue: reuse Loro, don't re-implement it — and don't read it back.** Per the oracle-design failure-correlation model (`~/.claude/skills/property-based-testing/reference/oracle-design.md`), the worst oracle isn't "reuses production code" — it's **correlated duplication of the thing under test** (and, equally bad, **reading SUT state back into the ref**, which is vacuous-by-construction). In the *composed E2E* PBT, Loro's CRDT correctness is **not under test** (it has its own tier); what's under test is integration — op construction, routing org/UI edits into the CRDT, projection drift. So the coupled core may legitimately **hold its own `LoroBackend` instance and drive it from *intent*** (the ops the transition sequence intended) to compute the full expected merged content, while the SUT feeds *its* Loro through the whole stack. Same engine, **independent derivation paths in** → uncorrelated on the integration axis; blind only to Loro-internal bugs, which are out of scope here. Conditions to keep it a "good 20" and not the archetype-40 trap: (1) not-under-test in this test; (2) fed from intent, never the SUT's produced ops; (3) the two paths into Loro genuinely differ. The alternative to reuse is *worse*: refusing it forces either lost precision (drop to a relational check) or the read-back cheat (Independence → 0).

**The shortcut that avoids the engine entirely (footprint partition).** Two concurrent edits with **disjoint footprints** commute — any sane sequence CRDT preserves non-overlapping concurrent edits — so the core predicts the exact merged value by applying both in any order, *without invoking Loro and independent of its algorithm*. Only **overlapping** footprints need the reused engine or a convergence relation. Footprint-disjointness is cheaply decidable from the ops, so bias the generator toward the commuting interior (high-precision construction oracle) and sample the conflicting residue separately under the weaker oracle — the difficulty moves out of the oracle and into generation.

**Classify each shared field by merge rule, not by subsystem.** Most block properties are **LWW-registers** (`task_state`, scalar props): the core predicts them exactly by tracking `(value, Lamport-ts)` and merging as max — no engine, no convergence. Only **sequence/text** content under concurrent overlapping edits needs the footprint partition. Audit which shared fields are which: it tells you precisely where the merge is real and where the coupled core already handles it for free.

**Structure consequence (refines, doesn't reverse, §5.1's "never a union of parts").** A composite Ref `{ core, org_ext, ui_ext, … }` is fine **iff** the extensions hold only disjoint subsystem-*private* data (org: paths/on-disk bytes/dirty-state; ui: caret/focus/scroll) and all shared block data lives in `core`. The interaction "override" the naive design reaches for (`apply_block_modification` reconciling two copies) collapses into "the core applies ops in order"; the only genuinely joint computation is the overlapping-conflict merge, and that is computed (engine-from-intent) or constrained (convergence), never stored in two places.

**Why this does not violate §5.2.** §5.2's "zero in-body variant projections" is about the Ref **degrading on capability/subsystem *absence*** (a config twin). Branching the Ref on an edit pair's *footprint* (disjoint vs overlapping) is branching on the **input**, not on the active-subsystem set — it is the oracle computing its answer, the same as any data-dependent reference logic. The §5.2 invariant (no config-variant projections) is untouched.

### 5.5 Open module contribution: a coupled core is not a closed struct (2026-06-26)

§5.1 ("monolithic by necessity") is about *correctness* — shared data must be single-homed. It is **not** an argument for a *closed* `ReferenceState`, and it should not be read as one. There is a separate, legitimate design axis the coupling rule says nothing against: **open contribution** — letting a new subsystem module (imagine `Loro+Iroh` P2P sync as a *not-yet-existing* module) drop in its **generators + transitions + reference-state + SUT impl** and integrate with *zero edits to a central type* — ideally shipped as a separate crate / git submodule. The monolithic `ReferenceState` struct is what blocks that today, and that is an *implementation* fact (hardcoded fields), not a consequence of §5.1.

**The corrected claim.** "One coupled core" forbids exactly one thing: splitting *shared* state across multiple owners (two copies of block content → reconcile-on-read, §5.4). It does **not** forbid an *open registry of private extensions* over that core. So the production form of the Ref is:

> **one coupled core** (all cross-cutting shared data — block tree, content, editor, focus — single-homed) **+ an open registry of per-module private-state extensions** (P2P: peer set, per-peer divergence, sync clocks).

The core doesn't compose; the *extensions* do. This is §5.4's `{ core, org_ext, ui_ext }` made into an *open registry* rather than hardcoded fields — symmetric to how the SUT is already an open `CapMap` of components.

**Most of the contribution is already open; the ref half is half-built.**
- **Transitions/generators/invariants are already capability-bound.** A module's transition is `impl TransitionRef<R> for PeerEdit where R: RefBlockTreeMut + RefPeersMut` — it programs against the core's *stable cap interface* + its own new caps, never naming the concrete Ref. The dispatch chokepoint (the closed `E2ETransition` enum) is exactly what §8.9 `cap_transition!` + the `inventory`/`typetag` open-registry PoC is built to flip.
- **The SUT is already an open `CapMap`** — a module's SUT impl is one more component.
- **The ref is already a `CapProvider`** (§8.8: `impl CapProvider for ReferenceState`, registered under each `Ref*` cap). So the ref side is already a typemap that *can* host multiple providers.

**What a module ships (the submodule shape):** its private reference-state type, registered into the ref's open extension registry (*not* a new field on `ReferenceState`); impls of its **own** `RefPeers`/`RefPeersMut` traits **for** `ReferenceState` (the orphan rule *permits* a local-trait-for-foreign-type impl, so a separate crate can do this); its capability-bound transitions/generators/invariants; and its SUT `CapProvider`. The coupling is preserved because the **core stays one provider owning all the cross-cutting caps** (block+editor+focus from one object — the same way a SUT component backs several caps from one `Arc`); the module's providers are orthogonal additions.

**The module hands *intent*; the core computes the merge.** When `SyncWithPeer` lands remote ops on the shared block tree, the module hands *intended* ops to the core's stable write interface — it does **not** compute the merge. The core's commute / LWW / cite-Loro-from-intent machinery (§5.4) produces the merged value. So a module needs *zero* knowledge of merge semantics, and the open-extension story composes cleanly with the merge-oracle story.

**The principled boundary (where the submodule dream legitimately stops).** This works for subsystems whose new ref-state is **private/additive** — P2P qualifies (peers/clocks private; the block tree it syncs is *already* core). Same condition as §5.1's hidden-capability rule (don't generate the transition → the cap stays hidden). It does **not** work, and *should not*, for a module that changes the **shared semantics of existing core data** (e.g. block-level ACLs every subsystem must now honor): that genuinely edits the domain model and must touch the core. No registry can or should hide a *real* coupling. The line is "additive subsystem vs. shared-vocabulary change," which is principled, not a wart.

**Backlog (not built).** The remaining gap to the submodule story is (a) flipping the transition dispatch to the open registry (§8.9, in flight) and (b) replacing `ReferenceState`'s hardcoded private fields with the open extension registry (the core keeps its cross-cutting fields; only subsystem-*private* state moves into the registry). Until (b), a new subsystem still edits `ReferenceState` — the one remaining central chokepoint on the ref side.

---

## 6. What a new slice costs (the payoff)

`crates/holon-integration-tests/tests/memory_wide_pbt.rs` (target):

```rust
// Components are reusable; a slice is the component list + the StateMachineTest shell.
fn memory_wide() -> CapMap {
    Config::new().with(MemoryBackend::new()).with(InMemEditor::new()).build()
}
// StateMachineTest: apply drives the wide transitions on the concrete SUT;
// check_invariants block_on's run_selected(REGISTRY, &sut, &ref).
```

Three configs, one shared registry, selection does the rest:

| Config | Components | Selects |
|---|---|---|
| memory-wide | `MemoryBackend` (+`InMemEditor`) | block-tree + editor + cdc invariants (~8–12) |
| gpui + memory (degraded) | `MemoryBackend` + `GpuiFrontend` | + viewmodel/frontend; **+ "shows source" twin** (query absent); − "decompiled rows" |
| full E2E | `LoroBackend` + `TursoProjection` + `GpuiFrontend(real)` + `FileSyncBackend` | all 35 (≡ today's `general_e2e`/`gpui_ui`) |

`E2ESut` stops being a 2152-line god-type — its cap impls relocate onto the four components, and "E2E" becomes the full config.

---

## 7. Evidence already in tree

**PoC — `crates/holon-pbt-core/src/composition.rs` (5 green tests).** Permanent generic spine in place; throwaway toy caps/components in `#[cfg(test)] mod poc`. Proves:
- the async trait-object typemap round-trips, one `Arc` backs many caps, an async cap read works through `Arc<dyn Cap>`;
- selected-but-absent lookup panics loud (not `None`, not faked);
- `cap_invariant!` single-sources needs + lookups; bodies `.await` cap reads;
- positive + negative selection drives three configs from one registry; degraded/full twins are mutually exclusive;
- the sync→async `StateMachineTest` glue (`block_on(run_selected)`) and a `&self` write cap mutating through `Arc`.

**Sizing spike — `SutCdc` converted end-to-end, compiled green (`holon-pbt-core` + `holon-integration-tests --features pbt --tests`), then reverted.** Findings:
- `#[async_trait(?Send)]` makes `dyn SutCdc` object-safe (the blocker for `CapMap`) — confirmed via an `_assert_object_safe(_: &dyn SutCdc)`.
- Total change ≈ **5 lines**, **no caller changes, no `&mut→&self`**. `drain_cdc(&mut self)` stays as-is (apply-only); `CachingProxy` doesn't even `impl SutCdc` (inherent `cdc_in_flight_cached`), so its call site is unchanged.
- Extrapolated step-0 cost: ~3 lines per read-cap trait × ~15–18 traits ≈ **50–70 mechanical lines, incrementally compilable**, no logic churn. Per-trait watch-item: a cap whose `E2ESut` future is rejected by `?Send` (none seen).

---

## 8. Migration plan (incremental; keep the suite GREEN after each step)

**Step 0 — make the read-cap traits object-safe. ✅ LANDED.** Added `async-trait` to `holon-pbt-core`; the 10 read caps in `capabilities.rs` and their `E2ESut` impls (`sut_capabilities.rs`), the `CachingProxy` impls, and the ToySut test impls are all `#[async_trait(?Send)]`. `SutCdc` keeps plain `#[async_trait(?Send)]` (its `drain_cdc(&mut self)` is apply-only, not proxy-forwarded). *Gate met:* `cargo check -p holon-integration-tests --features pbt --tests` green.

**Step 1 — introduce `CapMap` + generic runner, E2E as the first config (parity gate). 🟡 SUBSTANTIALLY LANDED; full-`general_e2e` confirm outstanding.** What's in tree:
- `composition.rs` spine promoted (not yet driving prod, but compiled-in with 5 green PoC tests).
- **`#[holon_macros::capmap_adapter]`** (new proc-macro, `holon-macros/src/capmap.rs`) generates the read-cap union-adapter on `CapMap`: for each read cap it emits the `#[async_trait(?Send)]` trait, the `CapName` impl, and the forwarding `impl Trait for CapMap` (`&self` async → `self.expect::<dyn Trait>().method(args).await`; `&mut self` → `unimplemented!`). All 10 read caps carry the one-line attribute — the ~50 hand-written forwards are gone. Adding a read cap is now a single attribute swap.
- **Generic runner seam extracted:** `run_proxy_registry<S: WideProxyCaps>` (free fn in `invariant_runner.rs`) is the SUT-generic core — builds the caching proxy, dispatches every read-only proxy-body, returns findings. `E2ESut::run_invariant_registry_gated` now wraps it (settle → `run_proxy_registry(self, …)` → append the 4+1 `&mut self` self-bodies → `report_findings`). `WideProxyCaps` (the 10-cap union) is the bound; `native_self_invariants()` (focus/caret/text/window-focus + otel `inv-sql-budget`) stays `E2ESut`-only because those bodies need `&mut SutDriver`.
- **De-risk asserts (compile-only):** `_assert_capmap_hosts_proxy_bodies` (body list type-checks over `CapMap`) **and** `_assert_capmap_drives_runner` (the whole `run_proxy_registry::<CapMap>` seam monomorphises). Both green ⇒ a composed slice can call the exact runner the E2ESut path uses.
- *Gates met so far:* `holon-pbt-core` 24+2+1+5 tests green; `invariant_runner::tests` 7/7 incl. `native_runner_dispatches_exactly_the_registry` (dispatch set unchanged ⇒ extraction behaviour-preserving); **post-extraction fast-slice parity (2026-06-15): `storage_consistency_pbt` green 29.1s + `general_e2e_pbt_sql_only` green 133.4s (both 32 cases, exit 0).**
- **Still outstanding (the real parity risk):** full `general_e2e` (Full/Loro twin) confirm — too slow to land in-session (>580s) and carries known pre-existing reds (deletebackward/linkmark families), so an unattended run is low-signal without a baseline diff. Run attended before deleting any old path.
- **Deliberately deferred (not built):** a `RegistryHost` trait abstracting *settle + suite-selection* was scoped in Task #6 but **not** introduced — a `CapMap` memory slice's settle is fundamentally different (no async Loro→SQL projection to await) and its `ComponentSet`/`has_window` inputs differ, so a shared trait now would be speculative. `run_proxy_registry` is the honest minimal seam; `RegistryHost` waits until Step 2 reveals what the memory slice actually provides.

**Step 2 — add the memory-wide slice. 🟢 FIRST VERTICAL SLICE LANDED (2026-06-15, path B).** `crates/holon-integration-tests/src/pbt/memory_slice.rs` — a composed `CapMap` slice with a single `MemoryBackendComponent` (over `holon::api::MemoryBackend`) that provides the `SutBackend` cap, and runs **real registry invariant bodies** selected by **capability presence** via the γ runner (`run_selected`), with no Turso, no `min_sut`, no `Subsystem`, no E2ESut. Three tests green sub-second (`cargo test -p holon-integration-tests --features pbt --lib memory_slice`, 0.00s): the positive run over a real `MemoryBackend` tree, plus both §6 fast-localization gates — the slice *catches* an injected parent cycle and a `Source`-without-`source_language` corruption.
- **The key enabler (`BridgedInvariant<I>`, now permanent in `composition.rs`):** wraps a statically-typed body `Invariant<CapMap, CapMap>` into an object-safe `CapInvariant`, with its cap `Needs` declared as *data* beside it (the runtime analog of the body's `where S: …` bound). Bodies that ignore the ref (`check(&self, _: &R, sut)`) impose no bound on `R`, so `R = CapMap` needs **no** Ref-cap adapter — the two structural invariants (`no_parent_cycles`, `source_language_iff_source`) bridge today with only `SutBackend` hosted.
- **Why path B beats §8.6's "corrected sequence":** because the slice provides its **own** `SutBackend` over `MemoryBackend` and selects by hosted caps, it needs neither the `min_sut` reclassification (§8.6, would break `loro_backend_pbt`) **nor** a non-Turso `SutBackend` realization on E2ESut. The two selection mechanisms (legacy `Subsystem`/`min_sut` for E2ESut; γ cap-presence for composed slices) coexist; `loro_backend_pbt` is untouched.
- **Ref side now composes too (2026-06-15, same session).** `#[capmap_adapter]` learned to skip `#[async_trait]` for fully-sync traits (it forced it before), so the sync `RefBackend` (owned returns → already object-safe) carries the one-line attribute without rippling onto its existing `impl RefBackend for ReferenceState`. `CapMap` now hosts `dyn RefBackend`, and the slice runs `inv-blocks-match-ref/block_raw` (`R: RefBackend, S: SutBackend`) when a ref map is wired — proven by three more tests: deselected when no ref is wired (§2 negative containment, disclosed not faked), selected + passing when the ref matches, and *catches* a `block_raw`-vs-ref content divergence. The ref map is a fixture today; §5.1's omnipotent `ReferenceState` registered under each `Ref*` cap is the production form.
- **`no_orphan_blocks` added (2026-06-15, same session) via the §8.6 refactor.** Took the cleaner option: dropped `no_orphan`'s spurious `SutSqlProjection` bound by deriving the `block_raw` id set from `block_raw_snapshot()` (`SutBackend`) instead of `all_block_ids()` — identical id set (both read `block_raw`), one fewer cap, and one fewer SQL query on the E2E path. Body-only change (no `min_sut`/registry edit → E2E selection unchanged). Now in the memory slice (needs `SutBackend` + `RefBackend`); 6 tests green incl. a negative that *catches* a dangling parent. **Parity-gated:** `storage_consistency_pbt` green 17.6s + `general_e2e_pbt_sql_only` green 68.9s (both exercise `no_orphan` over real Turso). Full/Loro-twin `general_e2e` remains the only un-run parity slice (slow + known pre-existing reds), same status as Step 1.
- **The dynamic dimension landed (2026-06-15, same session): mutation-sequence PBT.** `memory_slice_mutation_sequence_preserves_invariants` (proptest, 48 cases, 0.06s) drives a random sequence of create/update/delete-leaf ops against the production `MemoryBackend` (SUT) and an **independent** reference model in lockstep, running the cap-selected invariants after every tick. The ref is computed from intent, never read back from the SUT, so `blocks-match-ref` + `no-orphan` genuinely cross-check the real store each tick — this is what makes the slice bug-*finding*, not just bug-*catching*. (Delete is leaf-only because `MemoryBackend::delete_block` doesn't cascade — deleting a parent orphans its children, a real divergence the invariants would otherwise flag.) `memory_slice.rs` is now **7 tests green sub-second**. Used a plain `proptest!` + apply-loop rather than `proptest_state_machine::StateMachineTest`; the latter remains the path once mutations need the shared transition/generator machinery.
- **`RefBlockTree` now hosts — the borrow-returning-cap gotcha is SOLVED (2026-06-15, same session).** The blocker was that `#[capmap_adapter]`'s forward read `self.expect::<dyn T>().method(args)`, and `expect` *clones* the `Arc` into a temporary — so a method returning a borrow (`RefBlockTree::block_content -> Option<&str>`) dangled off that temporary. Fix: `CapMap::expect_ref<C>() -> &C` hands out a reference rooted in `&self` (downcasts the stored `Box<dyn Any>` to `&Arc<C>`, derefs to `&C`); the macro's **sync** forward now uses it (async stays on owned-`Arc` `expect`, the proven path). One method on `CapMap` + a two-line macro branch unblocks the *entire* borrow-returning `Ref*` family — no per-cap special-casing. `RefBlockTree` carries the one-line `#[capmap_adapter]` (sync trait → still no `#[async_trait]`, `impl RefBlockTree for ReferenceState` untouched). Two new bodies bridge into the slice (both `SutBackend + RefBlockTree`): `inv-block-content-matches-ref/block_raw` (reads the borrowing `block_content`) and `inv-block-parent-matches-ref/block_raw` (reads `parent_of`). The latter closes a real gap — `inv-blocks-match-ref/block_raw` compares only `{content, properties}` (it **excludes the `Parent` facet**, since the wide E2E path remaps doc-level parents across id spaces) and `no_orphan`/`no_parent_cycles` only check the parent *exists* / the chain *terminates*, so a block re-parented under a *different-but-valid* parent slipped through; the pure-memory slice has no doc-id remapping, so `parent_id` is directly comparable there. **`memory_slice.rs` is now 11 tests green sub-second** — positives (caps select them, values match), a negative-containment (content invariant deselected when only `RefBackend` is wired, disclosed not faked), a content-divergence catch, and a re-parent catch that asserts *isolation* (the existence/termination/content invariants stay green — only the parent linkage diverged); the mutation-sequence loop now cross-checks content **and** parent via `RefBlockTree` every tick. Gate met: `holon-pbt-core` 24+2+5 tests green (the `expect_absent` fail-loud panic test still passes) and `invariant_runner` 7/7 incl. the `_assert_capmap_*` compile proofs — so the `expect_ref` switch is sound across all 10 SUT read caps, not just the new ref one. `RefBlockTree` is now meaningfully exercised across three methods (`block_content`, `all_non_seed_block_ids`, `parent_of`).
- **SECOND SUT COMPONENT LANDED — the §6 "memory-wide" config is real (2026-06-15).** `InMemEditorComponent` (`memory_slice.rs`) is a headless active-editor running the same byte-offset, UTF-8-boundary caret math as production's `holon-frontend/headless_editor_mirror.rs::handle_keystroke`, minus the `ReactiveEngine`/Loro coupling that ties the real one to Turso. It provides the `SutEditorMirrorRead` cap (interior-mutable `Mutex` so the one `Arc` is both driven through `&self` writes in apply and hosted as the read cap — §4.4). Both editor read caps now carry `#[capmap_adapter]`: `SutEditorMirrorRead` (sync, owned `Result`) and `RefEditorMirror` (sync; its `active_editor_text -> Option<&str>` is the **second** cap to exercise `expect_ref`). `memory_wide_with_editor()` is two `.with`/`.register` calls — the payoff. Two real registry bodies select on the new caps: `inv-editor-text-matches-ref` + `inv-editor-caret-matches-ref` (`SutEditorMirrorRead + RefEditorMirror`). **`memory_slice.rs` is now 16 tests green sub-second:** editor invariants *deselected* without an editor (negative containment), a two-component positive (SUT editor agrees with the reference after a multi-byte open+type), a live-text catch + a caret catch (off-by-one — the named `MoveCursor` byte/keystroke-conflation class), and a **differential op-sequence proptest** driving random type/delete/move ops against the production-parity SUT editor (`String` + byte caret) and an *independent* reference (`EditorModel`: `Vec<char>` + codepoint caret) in lockstep — different representations, so a byte/codepoint conflation in either surfaces as a divergence (a genuine differential test, not two copies of one impl). `structural_block_registry` → renamed `memory_slice_registry`. Gates: `holon-pbt-core` 24+2+5 green, `invariant_runner` 7/7 — the editor cap adapters don't ripple onto `E2ESut`/`ReferenceState`'s existing impls.
- **Remaining for a full memory-*wide* slice:** wire the editor's commit into `MemoryBackend` so `blocks-match` cross-checks committed content end-to-end; structural editor ops (split/join at the caret) where the block-tree and editor interact; `RefFocus`/`RefLayout`/`RefRender`/`RefWatches` owned-return caps (host directly, but each needs its SUT counterpart — `SutViewModel`/`SutRenderer` ⇒ a third component — to select anything); the degraded "shows source" twin (`sut_absent: [dyn SutQueryResults]`, needs a query-engine component).
- **Note on the SutBackend-only block-tree boundary:** that invariant set is saturated (existence/acyclicity/source-lang/content/properties/id-set/parent); further block-tree invariants bind caps the pure-memory store lacks or need an ordered-children cap (`children_by_parent` is a `HashMap` → children-*order* off the flat snapshot is root-order-flaky). This is why coverage growth now comes from *more components* (the editor above), not more ref caps on the backend.
- **SECOND SLICE LANDED — `loro_slice` over a real CRDT proves the §6 "free re-run" claim (2026-06-15).** `pbt/loro_slice/` wraps a real `holon-loro` `LoroBackend` in a `LoroBackendComponent` providing `SutBackend` (over `get_all_blocks`) **and** `SutLoroLog`. It runs the **same** `composed_invariant_catalog()` the memory slice runs — zero catalog duplication — and selection lights up all six block-tree invariants over the CRDT *for free* (a selection test, not an invariant-add), plus a Loro mutation-sequence proptest that cross-checks the CRDT against an independent model each tick. Two Loro-specific invariants were added to the shared catalog: `inv-loro-no-errors` (`SutLoroLog`; honest-`false` in the standalone CRDT, teeth in the fixture catch + the E2E counter) and `inv-loro-children-match-ref` (`SutLoroLog + RefBlockTree`; the CRDT's fractional-index sibling order vs the ref's document order — real teeth, runs live in the proptest). `loro_children_of` reads the tree's authoritative order via `list_children`; `loro_had_errors` is honest-`false` because a standalone tree has no `LoroSyncController` (§5.1 hidden capability). Also added the **E0b selection-regression guard** (`memory_slice_selects_exactly_the_full_catalog`): it asserts the memory slice selects exactly its 8 applicable ids and *discloses* the 2 Loro invariants as deselected — and it immediately caught the catalog growth when the Loro invariants landed, doing its job. All gates green: composed+memory+loro 28/28, `holon-pbt-core` 24+2+1+5, `invariant_runner` 7/7.

- **THIRD SLICE LANDED — `sql_slice` over a real Turso `BackendEngine` (2026-06-15).** `pbt/sql_slice/` wraps the production storage + IVM matview layer in a `SqlProjectionComponent` providing `SutBackend` (over `block_raw`, via the shared `parse_block_row`) **and** `SutSqlProjection`. It is explicitly the **storage-layer slice of the future `E2ESut` replacement** (§F2 convergence): the plan is to retire the monolithic `E2ESut`, so overlapping its SQL realization is the *point* — the component reuses `E2ESut`'s own `block_raw` queries so the eventual swap is clean, while being genuinely lean (no reactive engine / frontend / navigation / CDC). The engine is built via `create_test_engine_with_setup` + a block-CRUD `SqlOperationProvider` over `BLOCK_WRITE_TABLE` (structural block ops like `indent`/`split_block` live on `SqlBlockOperations`/`EventInfraModule` and are out of this slice's scope). All six block-tree invariants run over Turso *for free* (selection, not invariant-add) + a Turso mutation-sequence proptest. One SQL-projection invariant added to the shared catalog: `inv-block-content-matches-ref` (`SutSqlProjection + RefBlockTree`, the direct `block_raw.content` column read — distinct id from the `…/block_raw` typed-snapshot variant), triad via `FixtureSqlProjection`. The navigation/focus/watch members of `SutSqlProjection` are honest-empty (no navigation in this slice, §5.1). E0b updated to disclose the SQL invariant as deselected in the memory slice. All gates green: sql_slice 4/4 (incl. Turso proptest), composed+memory+loro triads, `holon-pbt-core` 24+2+1+5, `invariant_runner` 7/7.

- **FOURTH SLICE LANDED — `frontend_slice` over a real headless render pipeline (2026-06-15).** `pbt/frontend_slice/` stands up a **windowless** production `FrontendSession` + `ReactiveEngine` over a Turso `BackendEngine` via `holon_app::new_from_config_with_di` (the exact DI path the GPUI/CLI frontends use), seeded with an org file — no GPUI, no geometry, no display link. `HeadlessFrontendComponent` provides `SutRenderer` (the real `ensure_watching`→`snapshot`→`interpret_pure`→`view_model_to_snapshot` pipeline — faithful port of `E2ESut`'s render methods, reusing the now-`pub(crate)` shared helpers), `SutViewModel` (real `headless_error_node_count`; gpui-frontend-engine-specific methods are honest-`None` — this slice has a headless engine, not a separate gpui *frontend engine*), and `SutBackend` (over `block_raw`, so the block-tree catalog runs over this fourth realization too). `Config::with_arc` added for the once-built session. One frontend invariant wired into the shared catalog: `inv-viewmodel-no-error-widgets` (`SutViewModel`, no ref — a SUT-internal render-liveness property), triad via `FixtureViewModel`, running over the **real** rendered tree (a valid layout has 0 error widgets); teeth verified. The headless render path is **not** GPUI-window-flaky (that was window/geometry/focus-specific); `resolve_watch` polls until loaded with a 3s ceiling. The `SutLayout`/geometry + gpui-frontend-engine invariants stay `E2ESut`-only (no window). Catalog 11→12. All gates green: frontend_slice 2/2, composed triads 22, memory 19 (E0b updated), loro 3, sql 4, `holon-pbt-core` 24, `invariant_runner` 7.

- **FIFTH SLICE LANDED — `sql_loro_slice`, the combined SQL+Loro SUT (2026-06-15).** `pbt/sql_loro_slice/` is the only non-redundant consumer of `SutLoroTaskState`: it composes the **existing** `SqlProjectionComponent` (real Turso) and `LoroBackendComponent` (real Loro CRDT) — no new component — so `inv-task-state-storage-coherence` can cross-check a block's `task_state` across two independent storage realizations (SQL `json_extract(properties,'$.task_state')` vs Loro `properties["task_state"]`). `SutLoroTaskState` was hosted on `CapMap` (`#[capmap_adapter]`) and added to `LoroBackendComponent`. **Composition choice (honesty):** both components provide `SutBackend` and `CapMap` is one-provider-per-cap, so registering both fully would *silently* shadow one block store; the `sql_loro_wide` builder instead registers SQL fully (canonical block store) and Loro for **only** its `SutLoroTaskState` cap. The catalog invariant + `Needs` + the fixture-driven catch triad live in `composed/` (reused, not re-authored); the slice's `integration_tests.rs` is deliberately minimal — a positive (both stores agree ⇒ selected + passes) and a catch (stores diverge ⇒ caught) over the **real** combined SUT, reusing `sql_slice::builders::new_sql_engine` (hoisted out of the SQL slice's tests, no copy-paste) and driving no bespoke generator. Catalog 12→13; E0b updated. Teeth verified. Gates green: sql_loro_slice 2/2, sql_slice 4/4, memory 19 (E0b), composed triads 25, `holon-pbt-core` 24+2+1+5, `invariant_runner` 7.

- **Shared-generation attempt (F5) → folded into the F2 convergence (2026-06-15).** A first cut introduced a bespoke `SliceDriver` trait + CRUD generator to dedupe the per-slice mutation loops, but that **reinvented the write-cap mechanism** the design already specifies (§4.4/§5.1/§6: `apply` drives the concrete SUT through `SutBlockTreeWrite`/`SutTransitionTarget`, and a slice is the component list). It was reverted. The correct shared-generation is to reuse the existing `E2ETransition` + `aggregate_transitions` + `ReferenceState` by having each composed slice be a concrete SUT impl'ing `SutTransitionTarget` — i.e. **Step 1 / the F2 convergence**, started on the memory slice.

**Step 3 — backfill caching + retire `sut_capabilities.rs`.** Move per-tick memoisation onto the `CapMap` wrapper; delete the absent-sentinel tax as components absorb the real impls.

### 8.5 Step-2 audit (2026-06-15): `min_sut` is realization-coupled, not capability-coupled — the central Step-2 decision

Goal of the audit: for a minimal memory-wide slice (`ComponentSet {BlockTree, Cdc, EditorState}`), which read caps must `CapMap` host, and what does `MemoryBackend` (`crates/holon/src/api/memory_backend.rs`) supply?

**`MemoryBackend` surface** is a pure block-tree store + change stream: `get_block` / `get_all_blocks(Traversal)` / `list_children` / `create|update|delete|move_block` / `watch_changes_since(StreamPosition) -> Stream<Change<Block>>`. No Turso, no Loro, no ViewModel/Renderer/Layout/Org. So it natively backs **BlockTree** (+ a **Cdc**-shaped change stream); the other subsystems are absent.

**The blocking finding.** Filtering the registry by `min_sut ⊆ {BlockTree, Cdc, EditorState}` selects almost nothing — only `inv-no-errors` (`[Cdc]`), `inv-active-watches-match-ref` (`[Cdc]`), `inv-editor-text-matches-ref` (`[EditorState]`). Every *structural block* invariant is excluded because its `min_sut` carries **`TursoProjection`**:
- `inv-no-orphan-blocks` `[BlockTree, TursoProjection]`, `inv-no-parent-cycles` `[BlockTree, TursoProjection]`, `inv-source-language-iff-source` `[BlockTree, TursoProjection]`, `inv-blocks-match-ref/block_raw` `[BlockTree, TursoProjection]`, `inv-live-children-match-ref` `[BlockTree, Loro, TursoProjection]`, …

But inspecting the **bodies**, these read **`SutBackend`** caps — `sut.live_block_snapshot()` (no_orphan_blocks) and `sut.block_raw_snapshot()` (no_parent_cycles) — *not* `SutSqlProjection`. The `TursoProjection` in `min_sut` is there only because **`E2ESut`'s realization of `SutBackend` reads Turso**: `live_block_snapshot` snapshots the CDC `live_blocks` matview mirror; `block_raw_snapshot` runs `query_sql("… FROM block_raw")`. The capability called is backend-level; the *storage it happens to read* is Turso, and that leaked into the declaration.

So the structural block invariants are **capability-portable in principle** — a slice can satisfy them by realizing `SutBackend::{live_block_snapshot, block_raw_snapshot}` over a non-Turso store. But "portable in principle" is **not** "safe to reclassify now" — see the correction below.

**Decision for Step 2 (original recommendation — now SUPERSEDED, see §8.6): reclassify `min_sut` from realization to capability.** Drop `TursoProjection` from invariants whose bodies call only `SutBackend` (no_orphan_blocks, no_parent_cycles, source_language_iff_source → `[BlockTree]`). Keep `TursoProjection` where the body genuinely reads a `SutSqlProjection` method or asserts projection-consistency (`inv-matview-consistent-with-ref`, `inv-blocks-match-ref/matview`, `inv-navigation-focus`, `inv-focus-roots`). This is the concrete resolution of §9's open "unify on the capability" question — and it is the same kind of *parse-don't-validate* boundary fix the project favours (declare the logical need, not an incidental storage path).
- *Risk (UNDERSTATED — see §8.6):* the audit claimed this "does **not** change blessed-slice selection (E2E carries `TursoProjection` anyway … `sql_only`/`storage` presets unaffected)". **That is wrong.** It overlooked the `loro_backend_pbt` slice.
- *Alternative (heavier, rejected as first move):* give the memory slice a real `TursoProjection` component mirroring `MemoryBackend`. Keeps `min_sut` untouched but defeats the point of a *pure-memory* fast slice and re-introduces the async-projection settle the memory path is meant to avoid.

**Caps the reclassified memory slice must host on `CapMap`** (everything else stays absent — no selected body calls it, so `expect::<dyn …>()` is never reached, exactly the §5.1 hidden-capability model on the SUT side): `SutBackend` (over `MemoryBackend`), plus the `Cdc`/editor write caps that `apply` needs (these live on the concrete slice SUT, not the Arc map — §4.4). `SutWatchRows` only if `inv-active-watches-match-ref` is kept in-slice.

### 8.6 Step-2 verification (2026-06-15): the reclassification is NOT selection-safe — `loro_backend_pbt` breaks it

Re-verifying §8.5's central recommendation against the code before editing the registry surfaced a parity-breaking error in the audit. **Do not reclassify `min_sut` to `[BlockTree]` as a standalone step.**

**The error.** §8.5 asserted the reclassification is "selection-safe for ALL existing slices" because "every existing slice wires Turso." Two facts falsify this:
1. **`BlockTree` is always-on** (`registry.rs::subsystems`, line 78 — "intrinsic observer, present in every run"). So `min_sut = [BlockTree]` ⟹ the invariant is selected by **every** slice, including non-Turso ones.
2. **A non-Turso slice is actually run.** `tests/loro_backend_pbt.rs` drives `ComponentSet::loro_vm_fast()` = `Wiring::loro_backend()` = `[Loro]` storage only. `state_machine.rs::storage_selector_for_wiring` maps Loro-only → `StorageSelector::LoroMemory`, which `di/lifecycle.rs` builds with **"no Turso connection"**. The existing test `registry.rs::scoped_set_checks_fewer_subsystems` even *asserts* `subsystems(loro_vm_fast)` contains no `TursoProjection`.

**Why it breaks.** All three bodies read the SUT through `SutBackend::block_raw_snapshot()` (and `no_orphan` also `live_block_snapshot()`). E2ESut's *only* `SutBackend` realization (`sut_capabilities.rs`) executes these as Turso SQL (`query_sql("… FROM block_raw")`) / a CDC matview read. On the `LoroMemory` SUT there is no connection and no `block_raw` table, so the queries fail. Today these invariants are correctly gated **out** of `loro_backend_pbt` by the `TursoProjection` in their `min_sut`. Reclassify to `[BlockTree]` and they get selected there and panic. (Note: this kills the "two clean ones" idea too — the clean-vs-not split I drew on the *trait bound* `SutSqlProjection` is irrelevant; the parity risk is `block_raw_snapshot` itself being Turso-realized, which all three share.)

**Root cause (refines §8.5, doesn't reverse it).** `min_sut` genuinely *is* realization-coupled — but the realization coupling is real, not nominal: there is exactly one `SutBackend` impl and it needs Turso. The destination (`[BlockTree]`) is right **only once a working non-Turso `SutBackend` realization exists on every slice that would then select these invariants.** The reclassification must therefore follow, not precede, that realization.

**Corrected Step-2 sequence** (supersedes the §8.5 list):
1. **Provide a non-Turso `SutBackend` realization first.** Either (a) make E2ESut's `SutBackend` read from the Loro store / `BlockQuerySource` when on `LoroMemory` (so `block_raw_snapshot`/`live_block_snapshot` work without Turso), or (b) build the memory `Component` (`MemoryBackend`) with its own `SutBackend` cap and *only* reclassify once the new slice — not the existing E2ESut loro slice — is the consumer. **Open decision — see the choice posed to the user.** Until one lands, `loro_backend_pbt` is the blocker.
2. **Then** reclassify `min_sut` → `[BlockTree]`, and update `scoped_set_checks_fewer_subsystems` / run `loro_backend_pbt` + the full parity gate. The change is only honest once every newly-selecting slice has a backend that answers the call.
3. Production `Component` wrapping `MemoryBackend` providing `SutBackend` + the Cdc change-stream cap (if not already done in step 1b).
4. `StateMachineTest` shell building the `CapMap`, restricting the `ComponentSet`, driving `run_proxy_registry` (the Step-1 seam, generic over `S: WideProxyCaps`).
5. Degraded "shows source" twin (`sut_absent: [dyn SutQueryResults]`) for the gpui+memory config.
6. *Gate:* new slice green + sub-second; reproduce a known structural bug on it to prove fast localization.

---

## 8.7 Subsystem-config shrinking (architectural delta-debugging) — spike landed 2026-06-16

The γ typemap turns the *active subsystem set* into **test input that proptest can
shrink**, so a failing case auto-minimizes to the minimal `(set of subsystems,
transition sequence)` that still reproduces — "fails with `{Loro}` only" vs "fails
with `{EditorState}` only" vs "fails regardless, empty set". Spike module:
`crates/holon-integration-tests/src/pbt/subsystem_shrink.rs` (run `cargo test -p
holon-integration-tests --features pbt --lib subsystem_shrink`).

**Why this matters — it replaces the unit-test base (proof-partition framing).** The
North Star inverts the test pyramid: the ONE configurable `WideE2E` PBT is the
primary asset, not a thin slow cap. Inverting it would forfeit the pyramid base's
fast feedback / localization / cheap repro — *except* that minimization recovers
them top-down. A wide failure shrinks to the minimal `(subsystem set, transition
sequence)`; that minimized config runs **only the required subsystems and
transitions**, so it executes at near unit-test speed and localizes to the culprit,
while being a *projection of the one PBT* (not a fork that can drift or trivialize).
Persisted as a capture (the shrunk *sequence*, not an RNG seed — ADR 0009 / §"Captures,
not seeds" in `PbtSlicing.md` §13), its replay is the deterministic sub-second gate
that *is* the recovered unit test — provably non-vacuous because it reproduced a real
failure. So we don't build a unit-test base; the harness *discovers* one per failure
(minimize) and *freezes* it (capture). Full framing: the property-based-testing skill,
`reference/strategy-and-pyramid.md` ("The proof-partition model").

**The `(Config, sequence)` model.** The config is part of the
`ReferenceStateMachine::State`, not a fixed slice choice. It is a
`BTreeSet<Subsystem>` over the *existing* `invariants::registry::Subsystem` enum
(not a fork, not a struct-of-bools), generated in `init_state` with
`proptest::sample::subsequence` over an env-scoped optional universe
(`HOLON_PBT_SUBSYSTEMS`, default `loro,editor`). `subsequence` shrinks toward the
shorter subsequence, so a present subsystem shrinks toward absent — minimization
direction is "fewer subsystems = the minimal causal set" for free. `init_test`
builds the `CapMap` *from* the generated set; `check_invariants` runs the **same**
`run_selected(&composed_invariant_catalog(), …)`, so selection lights up exactly
the applicable subset.

**The axes are REAL components, not fixtures** (no throwaway scaffolding that F6
would delete): `Loro` ⇒ a real `LoroBackendComponent` (the store *is* a live Loro
CRDT; `SutBackend` + `SutLoroLog`), absent ⇒ a real `MemoryBackend`; `EditorState`
⇒ a real `InMemEditorComponent`. `BlockTree` (a real store) is the always-on
substrate. Two real optional axes = a genuine *subset* to minimize, not one bool.

**Real components are correct, so planted bugs are wrong *reference* data** (the
same technique the catch triads use — never a faked component): `loro_order`
reverses the reference's two children → `inv-loro-children-match-ref` fails iff
real Loro is wired; `editor` diverges the reference editor text → editor invariant
fails iff the editor is wired; `content` diverges reference content → block
invariants fail regardless. The reference is built from the store's *returned* ids
so the Loro stable-id ↔ reference-`EntityUri` mapping is the identity.

**Config-in-state + precondition replay is the mechanism — and it works.** The
make-or-break unknown (is `init_state` shrinking mature enough to *invalidate
downstream transitions* during replay?) is resolved **yes**: the editor
transitions (`Type`/`Delete`) gate on `EditorState` in `preconditions`, and their
SUT `apply` panics if ever run without a wired editor (a tripwire). The
`loro_order`/`content` plants shrink `EditorState` **off**, yet across the entire
shrink the tripwire fired **zero** times — every removal correctly dropped the
now-invalid editor transitions. None of the handoff's three fallbacks were needed.

**Evidence — proptest minimized counterexamples (`cases: 128`):**
- `loro_order` → `{BlockTree, Loro}`, `ops: []`, transitions `[]`. Loro retained
  (causal), editor dropped (irrelevant).
- `editor` → `{BlockTree, EditorState}`. Editor retained, Loro dropped.
- `content` → causal minimum is `{BlockTree}` (proven deterministically by the
  `content_bug_is_independent_of_the_optionals` regression test). The proptest
  *greedy* shrink lands at `{BlockTree, EditorState}` (it drops Loro but not the
  causally-irrelevant editor) — a greedy-not-ddmin artifact, see "Greedy ≠ ddmin"
  below and the open question in §8.8.

This isolates a distinct minimal subset per bug across the powerset of a
two-element real-subsystem universe — the real "minimal causal *set*", not a bool.

**The evidence is *committed green tests*, not manual failing runs.** A
`regression` module hand-drives the shared `evaluate` seam to assert the causal
structure deterministically (`loro_order` fails iff Loro / passes on memory /
editor irrelevant; `editor` fails iff editor; `content` fails for every config;
`none` green for the whole powerset), plus `selection_follows_config` (the
subsystem-specific invariants run iff their subsystem is wired — non-vacuity) and
`editor_transitions_gated_on_editor_state` (criterion 4 at the precondition
level). A `universe` module unit-tests `parse_universe` (incl. fail-loud on an
unknown name) and that `init_state` exercises each optional on and off.

**Greedy ≠ ddmin (state honestly).** proptest shrinking is a greedy hill-climb,
not full delta-debugging: you get a *local* minimal config — in practice the right
culprit, but not a provable global minimum. Do not sell it as exhaustive.

**Cost-asymmetry scoping rule.** Loro/memory/editor are cheap to rebuild per case;
Turso/Org and frontend/GPUI real-window are not. The `HOLON_PBT_SUBSYSTEMS`
universe bound is the lever: keep the *default* universe to in-process subsystems
(cheap, the mechanism stays provable), widen it deliberately only once an expensive
subsystem is worth its per-case cost — no code change. Also avoid Org-off
normalization cross-cuts by preferring editor-mirror invariants while proving the
mechanism (the spike does; it never commits editor text, so committed-content
invariants stay at the consistent seed).

---

## 8.8 Oracle integrated onto the real `ReferenceState` — 2026-06-16 (pm)

The spike's first cut used a **bespoke** oracle (`SpikeRef` + a hand-rolled
`RefEditor`) — a parallel reference model, exactly the kind §5/§6 want to retire.
That oracle is now **swapped for the production `ReferenceState`**. This is the
reusable keystone for the whole F2 convergence, not a spike-local detail.

**The keystone: `impl CapProvider for ReferenceState`.** `ReferenceState` already
implements every `Ref*` trait; the missing piece was a `CapProvider` so it can
*be* the ref `CapMap` that `run_selected` consumes. Added in
`reference_capabilities.rs`, plus `reference_state_ref_caps(Arc<ReferenceState>) ->
CapMap`. It registers exactly the read caps the catalog consumes today
(`RefBackend` + `RefBlockTree` + `RefEditorMirror`) — the *same* surface
`FixtureRef` + `FixtureEditorRef` expose, so invariant **selection is identical**
to the fixture path (no catalog scope creep). Selection is an AND over the SUT
*and* ref cap sets (`Needs::selected_against`), so registering ref caps
unconditionally is safe — the editor invariants still deselect when the SUT has no
editor.

**State = `ReferenceState`.** `init_state` maps the generated `subsequence` →
`Wiring::custom([Loro?], [], [UI?])` → `started_reference_state(wiring)` (app
started + a minimal seed-classified layout query block to satisfy
`is_properly_setup`) → seeds `parent/c1/c2` with **fixed shared ids**
(`block:parent|c1|c2`; both `MemoryBackend` and `LoroBackend::create_block` honor a
provided id, so no store-returned-id round-trip — the ref tree is fixed at
generation time, before the SUT store exists). The editor is opened **only when
`wiring.has_actor(UI)`** — opening it unconditionally would let editor transitions
generate for `{Loro}`/`{}` configs and trip the editor-less-SUT tripwire.
*Gotcha:* `current_focus(Main)` reads `navigation_history`, **not** the
`focused_entity_id` that `set_focus` writes — the bootstrap must push a nav-history
entry (as production `NavigateFocus` does) or the editor preconditions silently
fail and typing never fires.

**Mirror-only `apply`; plants injected at observation.** The ref-side `apply`
calls `RefEditorMirrorMut::{type_chars,delete_backward,move_cursor}` **directly** —
never the production `*_apply_to_ref`, which commits the typed text into block
content (the SUT's `InMemEditorComponent` is detached and never writes the store, so
a committing ref would diverge). Plants are injected into a **clone** of the ref
state inside `check_invariants`, so the live proptest state stays correct across the
whole transition sequence.

**Teeth — the integration is not vacuous.** Temporarily breaking
`editor_caret::insert_at` (drop the caret advance) turns the *default `none`-plant*
run RED on `inv-editor-caret-matches-ref`, with the reference editor showing real
multi-byte typed text (`"中😀中"`, `"c1€"`). This proves two things at once: editor
transitions **actually fire** with real content (not all-`Touch`), and the
reference's independent-codepoint math genuinely cross-checks the SUT's shared
`editor_caret` (the byte/codepoint bug class). Stronger than `selection_follows_config`,
which only proves the invariant is *selected*.

**Editor gating is now purely capability-based — two env side-channels removed.**
The editor-transition preconditions gate solely on `has_editor_buffer()` (the
capability, = `wiring.has_actor(UI)`). Consequently:
- `PBT_ATOMIC_EDITOR` / `RefLifecycle::atomic_editor_enabled()` was **dead** and is
  removed (trait method + all impls + the env static + `enable_atomic_editor_if_unset`
  and its callers). The capability *is* the gate.
- `PBT_REAL_EDITOR` / `ReferenceState::real_editor_enabled()` had one live effect —
  commit-on-blur in `blur_active_editor` — now a `ReferenceState.real_editor: bool`
  field set by the real-editor driver harness (`phased.rs`), which builds the ref
  state directly. No process-global env; deterministic and capture/replay-faithful
  via the construction path. `fixture::CAPTURE_ENV_FLAGS` shrinks to
  `["PBT_MUTABLE_TEXT"]`.

**Open question (shrink quality).** The `content` plant's greedy shrink retains the
causally-irrelevant `EditorState` (lands at `{BlockTree, EditorState}`, not the
deterministic causal minimum `{BlockTree}`). Worth checking whether
`proptest-state-machine` shrinks `init_state` (the subsequence) at all for failures
that reproduce *before* any transition — that is precisely the architectural-delta
minimization this PBT sells.

**Next steps (toward F2), each unblocked by this keystone:** (1) committed-content
parity — the deferred half of mirror-only (per-keystroke/on-blur commit on both
sides, the `block_raw` editor invariant, matched normalization); (2) structural
transitions (Split/Join/Indent/Outdent/Move) — now viable since `ReferenceState`
carries a real tree + honest `apply_to_ref`; (3) widen the universe past in-process
subsystems; (4) migrate the five slices off `FixtureRef`/`FixtureEditorRef` onto
`reference_state_ref_caps`, then **delete `FixtureRef`/`EditorModel`/`EditorPureRef`**;
(5) retire `E2ESut`. See the Backlog.

---

## 8.9 Authoring seam: `cap_transition!` — decouple *how a transition is written* from *how it's dispatched* (2026-06-22)

The closed `E2ETransition` enum is the right **dispatch** substrate *today*: it monomorphises
the same `impl<S: SutHandle> TransitionImpl` to both `E2ESut<V>` (the live `general_e2e_pbt`)
and `&mut CapMap` (the `general_e2e_composed_pbt` swap), and a `Box<dyn Transition>` could not —
it would have to erase to one SUT type while the two coexist through E5. But "closed enum" is a
property of the central `declare_e2e_transitions!` macro + the generic dispatch — **not** of the
52 per-transition files. Keeping the *authoring surface* behind a macro keeps the open-vs-closed
choice **reversible** instead of baked into every file. This section is the standing answer to
the implicit "closed enum forever" reading of §9: the enum is kept *for now, behind a seam we
can flip*, not as a permanent commitment.

**`cap_transition!`** (in `transition_dispatch.rs`) is that seam. It generates a transition's
`TransitionImpl<ReferenceState, S>` block — bound on exactly one fine-grained cap — **and** the
matching `required_caps()` (`declared_caps()`) from the *same* cap token. Two forms:

```rust
// single cap: required_caps() body becomes `Self::declared_caps()`
cap_transition! {
    SplitBlock: SutBlockTreeWrite,
    |me, _state, sut| { sut.apply_split_block(&me.block_id, me.position).await; }
}
// no cap: bound on the full SutHandle bundle; required_caps stays the empty default
cap_transition! { Nothing, |_me, _state, _sut| {} }
```

**Immediate payoff (landed).** The cap is stated **once**, so `required_caps()` and the
`S: Cap` dispatch bound cannot drift → a migrated transition needs **no entry** in the
`required_caps_match_transition_impl_bounds` guard test. `split_block` (single-cap) and
`nothing` (no-cap) are migrated; both guard entries removed; suite green. Migrating the rest is
mechanical, after which the guard test is deleted outright.

**Seam payoff (future, reversible).** The body calls `sut.<cap-method>(…)`, which type-checks
identically whether `sut: &mut S` (`S: Cap`, today) or `sut: &mut CapMap` (`CapMap` implements
every cap via `#[capmap_adapter]`). So retargeting the macro's expansion later — e.g. to a
`#[typetag::serde]` + `inventory` open registry once `CapMap` is the **sole** SUT post-E5 —
changes only this macro's body, **never** the transition files. The dispatch decision becomes a
one-macro flip (optionally a cargo feature gating the two backends), not a 52-file rewrite.

**What it deliberately does NOT do.** It does not replace the enum or `aggregate_transitions`;
closed dispatch stays (load-bearing while E2ESut and CapMap coexist). The seam only makes the
eventual decision cheap and local.

**Adjacent moves it unlocks (options, not commitments):**
- migrate the remaining single-cap transitions → delete the drift-guard entirely;
- a `build.rs` glob of `transitions/*.rs` to generate the `declare_e2e_transitions!` variant
  list — removing the *last* central edit (drop a file = a new variant) while keeping closed
  dispatch, native async, serde and exhaustiveness. This is the "open authoring + closed
  dispatch" point neither the enum nor a full open registry occupies;
- key the bisect/replay record (ADR 0009) on `variant_name()` rather than the enum's derived
  serde, so a later typetag flip doesn't invalidate the saved counterexample corpus.

**Worked reference for the open encoding.** `experiments/open-registry-poc/` is a standalone,
runnable PoC of the fully-open `Box<dyn Transition>` + `inventory` + `typetag` encoding (with the
`CapMap` SUT, cap-gated alphabet+invariants, and §8.7 shrinking). Its README carries the
**staged migration path** (Tier 1 = `cap_transition!`, landed; Tier 1.5 = `build.rs` glob for
open-authoring/closed-dispatch; Tier 2 = the open encoding, which needs E5 because a trait object
must erase to a single SUT type and `E2ESut`+`CapMap` coexist until then). Treat it as the
reference for *if/when* we flip the seam — not as a plan of record.

---

## 8.10 Convergence rule — discharge an `E2ESut` cap consumer by DELETION, not by a new composed slice (2026-06-25)

This is the standing answer to a drift the E-track is prone to, and it overrides the
mechanical instinct of the earlier E3 increments. **State it once, follow it always.**

**The North Star (Backlog ★) has exactly one surviving PBT:** the configurable
`general_e2e_composed_pbt` (`WideE2E`), parameterized by active subsystems. Every
`*_slice`, `*_pbt`, and per-cap `ComposedSlice` is **scaffolding scheduled for deletion**.
So the *only* progress that counts is **a capability landing in the ONE PBT and a slice
dying** — never a slice being *born*.

**The hazard.** E3 deletes an `E2ESut` cap impl, which is blocked while any test consumes
that cap over `E2ESut`. The tempting discharge is: rewrite that test's standalone slice as
a `ComposedSut<NewSlice>` (same boot, narrowed alphabet, the one invariant), then delete the
impl. That *does* unblock the deletion — but it **grows** scaffolding: the new slice is itself
on the deletion list, so a future session must migrate/delete it too. That is the
"temporary-PBT churn loop" — discharging temporary work by minting more temporary work.

**The rule.** Before discharging an `E2ESut` cap consumer, ask: **does the swap config
(`full_headless`, the SUT `WideE2E` already drives) provide this cap?**

- **YES (the common case)** — the ONE PBT *already* runs the cap's invariants via the shared
  `composed_invariant_catalog()` selected by cap-presence. So:
  1. **Delete the standalone test outright.** Do **not** build a `ComposedSlice` for it.
  2. If the invariant must be *guaranteed* exercised (not merely selectable), add its id to
     `WIDE_REQUIRED_INVARIANTS` (`wide_e2e.rs`) — one line — so the ONE PBT runs it every tick.
  3. If the invariant has a real-SUT **non-vacuity / teeth** proof that only the deleted slice
     carried (e.g. "a real `ToggleState` moves *both* the SQL and Loro projection in lockstep"),
     relocate just that test into the invariant's own file
     (`composed/invariants/<name>.rs`) next to its fixture triad — its durable home,
     independent of any slice's lifetime.
  4. Then delete the `E2ESut` cap impl (E3) and record the `E1_RELOCATED` row in `parity.rs`.

  Net: **−1 test, −1 cap impl, +0 scaffolding.** `parity.rs` selection-parity is the static
  proof that deleting the standalone test loses no coverage — consult it, don't build a
  stand-in slice to "be safe."

- **NO** — `WideE2E` genuinely cannot drive this cap yet (the windowed/GPUI/E4 input family:
  `PressKey`/`ArrowNavigate`/drag, or any cap `full_headless` does not host). *Then* a focused
  `ComposedSlice` is justified, because there is no ONE-PBT coverage to inherit. Mark it
  explicitly as transient (it dies when E4 lands its component + axis).

**The "NO" branch: one SUT shape, two harnesses (`!has_actor(UI)` is permanent) (2026-06-26).**
The `SutLayout`/`SutDriver` "windowed shell" caps look fundamentally different from every other
cap because they need live geometry + real platform input (keymap, hit-test, drag bounds) that a
headless reactive tree has no equivalent for. `compose_sut` asserts `!has_actor(UI)` (`builder.rs`)
and the windowed checks build a composed `CapMap` from the live window's handles via
`compose_windowed_sut` (wrapping `window_input_wide`) inside `run_windowed_composed_check`. This is
**not** a permanent two-SUT-*shape* design — both paths yield a `ComponentSet`-described `CapMap`
that runs the one shared catalog via `run_selected`. `UserDriver` remains the single input interface
(`ReactiveEngineDriver` headless, `GpuiUserDriver`/`SimUserDriver` windowed — never forked);
`GpuiDriverComponent` only *wraps* whichever production `dyn UserDriver` the window installs and
adds a geometry precheck + cap gating. But the **construction entry does not unify**: `compose_sut`
boots on the tokio runtime, while a GPUI window has **thread affinity** — it must be launched on the
gpui thread by the windowed harness, which hands its handles to `compose_windowed_sut`. So
`!has_actor(UI)` is **correct and permanent** (it fail-louds the unbuildable headless path, pointing
at the sibling entry), and the windowed StateMachineTest **harness** is thread-bound and cannot fold
into WideE2E's tokio loop. End state (Backlog ★ "GPUI axis"): **one SUT shape + one catalog, two
harnesses** (headless tokio + windowed gpui-thread). The chevron verb gap is closed
(`UserDriver::set_block_expanded`, 2026-06-26). What remains is to repoint the windowed
transition-apply (`apply_split_block_input_pipeline_to_sut`, `engine_focused_block`, Gherkin
fixtures) off `<E2ESut as SutLayout+SutDriver>` onto the windowed `CapMap`, then delete E2ESut's
windowed **cap impls** — the windowed harness survives. See `PbtComposition_EndgameRoadmap.md`.

**Worked example (the rule's origin).** `task_state_coherence_pbt` consumed `SutLoroTaskState`
+ `SutSqlProjection` over `E2ESut`. The 2026-06-24 increment (jj `xk`) discharged it by minting
`TaskStateSlice` — but `full_headless` already hosts both caps and `WideE2E` already runs
`inv-task-state-storage-coherence` via the catalog, so the slice added **zero** coverage and one
deletion obligation. The 2026-06-25 cleanup applied this rule: added the id to
`WIDE_REQUIRED_INVARIANTS`, relocated the SQL↔Loro lockstep teeth into
`composed/invariants/task_state_storage_coherence.rs`, and deleted `TaskStateSlice` +
`tests/task_state_coherence_pbt.rs`. Same impl-deletion, no new scaffolding.

**Litmus test for any E-track increment:** *did total scaffolding (slices + standalone PBTs) go
DOWN?* If an increment leaves it flat or up, it is churn unless the cap is in the "NO" branch
above. Judge by deletions, not by green slices.

---

## 8.11 Layer-localization — the driver ladder IS a subsystem axis (no new dimension) (2026-06-26)

This connects the "drive interactions UI-adjacent" directive (the PBT drives user interactions
through the logic layer *just below* the UI surface, not an operation dispatch — even headless;
the UI is a thin shell) to the ADR 0009 subsystem minimizer, and answers: *can we localize a bug
to a **layer** — geometry vs view-model vs engine?*

**The ladder.** User interactions can be driven by three `UserDriver` implementations forming a
**total order of faithful refinements** — each = the one below + the production logic the lower
one shortcuts:

```
DirectUserDriver          raw OperationIntent → engine reducer                  (dispatch floor)
  ⊏ ReactiveEngineDriver    + find_click_intent resolution + InputState/MutableText editing
      ⊏ GpuiUserDriver        + geometry/hit-test + keymap + real platform-input pump
```

**Highest-available rule ⇒ it collapses to 1-D (the driver layers ARE subsystems).** The driver is
a *pure function of the active subsystem set*: `Actor::UI` present → `GpuiUserDriver`; ViewModel
present, no UI → `ReactiveEngineDriver`; neither → `DirectUserDriver`. So this is **not a new
dimension** — the layers are subsystems already in the `ComponentSet`, and a transition is always
driven against the **highest available** `UserDriver`. Three things fall out:
- The ladder is **well-ordered for free** by the existing validity constraint `Actor::UI ⟹ ViewModel`
  (`component_set.rs`): no window without a view-model, so subsystem bisection can only peel UI
  before/with ViewModel — the descent GPUI→VM→dispatch is automatic.
- Invariant + generator deactivation is the **existing** cap-selection (`Needs::selected_against` +
  the `aggregate_transitions` wiring gate): UI off deselects `SutLayout`/window-focus invariants and
  narrows UI-only gestures; ViewModel off deselects VM invariants. The same set now also picks the
  *driver*.
- **No new shrink machinery, no new cost.** The bisector already re-runs the trace at smaller
  subsystem sets; the driver simply descends with each peel.

**1-D is not just cheaper — it is more *correct*.** A 2-D axis (vary the driver independently of
subsystem presence) would permit *unfaithful* configs — "drive via dispatch while a window is
present." Production never does that (if a window exists, input flows through it), so such a config
can only fabricate bugs, never find real ones. "Highest available" enforces faithfulness: always
exercise the full stack that is present; *removing* a subsystem legitimately removes its layer. So
every config the bisector runs is one that can occur in production, and localization is a byproduct
of ordinary subsystem minimization.

**Localization = the boundary, across the bisection, where the *pinned* failure stops reproducing.**
Pin the original failure *signature* (invariant id + the specific divergence); count any *other*
failure during shrink as a non-reproduction (so a different bug the upper layer masked causes no
slippage — this is also the multi-bug safeguard). Layers ≥ L diverge from ref, layers < L match ⇒
the bug lives in the **L-delta**:
- gone when **UI** peels off → the **GPUI delta** (geometry / hit-test / keymap / platform input).
- gone when **ViewModel** peels off → the **ViewModel delta** (intent resolution / editor input).
- still failing at the **dispatch floor** → the **engine / backend** itself.

**Read-side and write-side localization compose.** The *read* side already localizes *within* one
run: one application, N projections compared to ref (the data-layer "trouble begins at" report).
The *write* side localizes *across* runs: the driver **is** the application, so you cannot observe a
single write at multiple layers — you re-run the trace with a lower driver. That across-run cost is
**already paid** by subsystem bisection. Net: data-layer localization per run (reads),
interaction-layer localization across the bisection (writes).

**Faithfulness is the load-bearing assumption — and it is path-dependent.** The ladder is sound only
where the lower layer runs the *same code* the upper one does, minus the delta:
- **Gesture / intent path — a faithful shortcut, not a copy.** `ReactiveEngineDriver::click_entity`
  reads the *same* `node.click_intent()` GPUI's `on_mouse_down` reads, and calls the *same*
  `dispatch_intent_sync` (the engine *is* `BuilderServices`). The only substituted step is *locating*
  the node — id-walk vs geometry hit-test — which is exactly the GPUI delta.
- **Editor keystroke path — a partial reimplementation (the soft spot).** Prod editor input flows
  through gpui-component's `InputState`; headless flows through `HeadlessEditorMirror`. They share
  the byte-offset math (`editor_caret.rs`) but not the `InputState` integration, so editor-layer
  localization is only as sound as the shared code. **This axis therefore *exposes* where the
  thin-UI architecture is unfinished** — the editor still has logic trapped in the GPUI widget.
  Making it fully sound = extracting that logic into a shared frontend layer both drivers run (the
  thin-UI directive, applied to the editor). The axis and the architecture co-evolve.

**Construction-time driver selection (parse, don't validate — the layer is chosen ONCE).** The
builder knows the full subsystem set, so it picks **exactly one** `UserDriver` and installs it as the
*single* driver backing the gesture caps in the `CapMap` (`Actor::UI`→`GpuiUserDriver`, ViewModel→
`ReactiveEngineDriver`, neither→`DirectUserDriver`). Gesture transitions bind the **driver** caps
(`SutDriver`/`SutBlockInteract`/`SutLayout`/`SutEditorMirrorWrite`), so *which layer runs* is decided
solely by which driver the builder installed — the lower drivers were never constructed for this run,
so a transition **cannot reach one** (it isn't in the map, and there is no ambient driver registry).
This replaces a runtime "prefer the higher cap" rule with construction-time choice: a single source of
truth for the layer, and the wrong layer made unreachable by encapsulation rather than discouraged.
Consequences: the dispatch floor stops being a distinct always-present `SutBlockTreeWrite` cap and
becomes *just another `UserDriver`* (a `DirectUserDriver`, installed only when no higher driver is);
and the bisector **descends layers by re-construction** — it hands the builder a smaller subsystem set,
the builder re-derives the single driver, transitions transparently use it (the bisector never needs to
know drivers exist). *Caveat — value-level, not compile-time:* the layer is runtime-selected from the
env-chosen set and the bisector runs many configs in one process, so the driver is inherently `dyn`;
the guarantee is "exactly one instance constructed & encapsulated per run," not a monomorphized type.
That is the same strength as the rest of the cap model (absence = the cap isn't there → the transition
narrows out), applied to *which driver* rather than *which subsystem*.

**Care-points (the discipline that keeps localization honest).**
1. The dispatch floor (`DirectUserDriver`) applies a structural op **directly to the engine**
   (`synthetic_dispatch` == `execute_operation`), below the geometry/view-model interaction layers —
   not by re-running production interaction resolution, else it isn't *below* the VM. **LL-2 verified
   (2026-06-26): the floor provides only the *structural-write* cap (`SutBlockTreeWrite`), NOT the
   UI-gesture caps.** `DirectUserDriver` wraps only a `BackendEngine` (no ViewModel/focus/geometry); a
   storage-only config has neither. UI-only gestures (click→focus, expand/collapse, slash) are
   view-model concepts that bottom out at the VM rung (care-point 3) — there is no faithful sub-VM
   "ref-intended effect" for them, so the floor legitimately does not host them.
2. **Install exactly one driver per run** (construction-time, above) — gesture transitions bind the
   *driver* caps, never a separate `SutBlockTreeWrite`/`OpDispatchWriter` dispatch cap. Installing
   *both* a driver and `OpDispatchWriter` for the same gesture, or binding a gesture to the dispatch
   cap while a driver layer is present, is the **rejected anti-pattern** (it tests the wrong layer —
   see the skill anti-patterns).
3. **View-local transitions (expand/collapse) correctly bottom out at the VM rung** — they narrow
   out when ViewModel is off, because there is no sub-VM layer for a view-local concept; that *is*
   the right localization.
4. **Non-gesture mutations** (`BulkExternalAdd` / `Peer*` / `ApplyMutation`) are layer-invariant
   (always direct) — "highest available" does not apply; they ride a separate cap.
5. **Localization is bounded by the failing invariant's caps.** A failure only a `SutLayout`
   invariant can catch cannot be localized below UI — the detector deselects there. "Bug gone at the
   lower layer" must be read as *gone **or** undetectable here*; the signature pin plus a check that
   the invariant is still *selected* is the discriminator.

**Operation → layer mapping (not uniform — it partitions, cleanly).**

| Transition class | dispatch floor | ViewModel | GPUI |
|---|---|---|---|
| Structural (Split/Join/Indent/Outdent/Move) | ✓ (the op) | ✓ | ✓ |
| Navigation (Focus/Home/Pin/Unpin; Back/Fwd headless-gappy) | ✓ (nav op) | ✓ | ✓ |
| Click / Drag / Slash / Arrow | ✓ (ref-intended intent) | ✓ | ✓ |
| Editor (Type/Move/Delete/PressKey/Blur) | ✓ (ref-effect set-content) | ✓ | ✓ |
| Expand / Collapse | ✗ (view-local — no engine op) | ✓ | ✓ |
| External/Lifecycle (BulkAdd/Peer*/Mutation/WriteOrg/StartApp…) | ✓ | — | — (layer-invariant) |

A transition that has no form at a layer simply narrows out there — the same per-transition
self-narrowing the wiring gate already does for absent subsystems.

**Relation to the in-flight work.** The `UserDriver`-backed input component (Backlog E-track) is the
**missing VM rung** — a headless `ReactiveEngineDriver`-backed driver inside `compose_sut`; a
`DirectUserDriver`-backed component is the **dispatch floor**. Once both exist behind the
highest-available rule, the existing bisector yields layer-localization with no new dimension.

---

## 8.12 The windowed `ComposedSut` realization — and the mixed-rung interim state (2026-07-01)

§8.10's "NO branch" text describes the windowed check as `compose_windowed_sut` (wrapping
`window_input_wide`) run per-tick beside a live `E2ESut`. That mechanism still exists (the E4
per-tick hook), but it is **superseded as the target** by the landed windowed `ComposedSut`
(roadmap "Round 5 UPDATE"), which this section promotes into the architecture doc:

- **Deferred-driver base:** `compose_sut_windowed_base[_seeded]` = the full headless
  `compose_sut` boot with the driver rung **deferred** (`DriverPlacement::Deferred`) — backend,
  storage, editor caps and the `IdResolver` reconcile all come from the one production booter;
  the window is a **pure renderer** over the same booted `session`/`reactive`.
- **Pure-insert overlay (whole gesture rung, updated 2026-07-02):** on the gpui thread,
  `overlay_windowed_caps(caps, frontend, geometry, engine, driver)` INSERTS `GpuiWindowComponent`
  (`SutLayout`) + the window-driver `DriverInputComponent` (`SutDriver`/`SutBlockInteract`/…) AND the
  gesture-write family (`SutBlockTreeWrite`/`SutFocusWrite`/`SutEditorMirrorWrite`/`SutMutate`) via
  `frontend.register_gesture_writes(caps, driver)` — the ONE routine (`register_gesture_writes`) that
  enumerates the family, shared with the headless base. No cap is registered-then-overridden: the
  `DriverPlacement::Deferred` base withholds the ENTIRE gesture rung, so every overlay call is a plain
  `insert` (fail-loud on duplicate, C-2), NOT `CapMap::replace`. The pre-window state is honestly
  capless — the generated alphabet auto-narrows the gesture transitions out until the window exists.
- **Injected-handle harness:** `ComposedSut::from_parts` (skips `init_test`'s tokio boot — the
  window has thread affinity, §8.10) + a `SettleHook` that pumps the window to a fixed point
  before each `check_invariants`; a windowed non-vacuity floor keyed off the ACTUAL `SutLayout`
  cap.
- **Cap-set-honest oracle:** the windowed oracle carries the LIVE SUT's cap set
  (`ComposedSut::cap_set()` → `wide_e2e_windowed_ref(cap_set)`), so `aggregate_transitions`
  narrows the alphabet to what the window genuinely drives — never `CapSet::without()` fakery
  (a faithful cap is present or the impl is fixed; withholding is an invalid intermediate state).

**Mixed-rung interim — now CLOSED for the write family (2026-07-02).** The former divergence was
that the deferred base registered the frontend's write caps over its *headless*
`ReactiveEngineDriver`, and the overlay `CapMap::replace`d them — a register-then-override that split
gestures across rungs (`ClickBlock` on the window driver, `NavigateFocus`/editor/structural on the
headless VM rung). The insert-only restructure removes this: the base registers NONE of
`SutBlockTreeWrite`/`SutFocusWrite`/`SutEditorMirrorWrite`/`SutMutate`, and `overlay_windowed_caps`
`insert`s the whole family bound to the *window's* driver via `register_gesture_writes` — so every
windowed gesture (writes included) rides the one highest-available driver, satisfying §8.11's
one-driver-per-run rule by construction. move_up/move_down keep the disclosed `OpDispatchWriter`
fallback inside the keystroke writer (mechanism 3), and the EXCLUDED classes remain excluded until a
window-driver mechanism exists. Tracked in §11 (C-3).

---

## 9. Risks, gotchas, open questions

- **Step 1 parity is the real risk.** Everything else is mechanical. Land it behind the existing `E2ESut` entry point and diff selection against the blessed slices before deleting the old path.
- **`?Send` per trait.** Watch for an `E2ESut` cap future that captures non-`?Send`-compatible state; none appeared in the spike but verify per trait.
- **Ref coupling.** The omnipotent core is monolithic *by necessity* — do not try to "union" *shared* Ref state from parts. Sub-Refs are projections, not parts. The subtlety (§5.4): "don't union parts" forbids two owners of one shared datum, *not* composing the Ref from disjoint subsystem-private extensions over a shared core. And when a shared datum's value is a genuine merge, the core computes it via a not-under-test merge engine fed from intent (or a convergence relation for the overlapping residue) — never by reading the SUT back, and never by re-implementing Loro. **"Monolithic by necessity" is about *shared data*, not the struct: the Ref may be an *open registry* of per-module private extensions over the coupled core (§5.5) — that is the seam for new-subsystem / git-submodule contribution, and the only thing it forbids is a module silently owning a second copy of a shared datum.**
- **Degraded twins must be routed through the `CapSet`** on both SUT and Ref, or false divergence (§5.2 soundness rule).
- **Caching adapter union impl** lives once on `CapMap`; do not let it regress into per-SUT panicking impls (that's the rejected Move B).
- **`CapMap::insert` is fail-loud on duplicate (FIXED 2026-07-02, C-2).** One-provider-per-cap is now enforced by the type: `insert` panics on a duplicate cap registration (naming the cap). The flip surfaced four intentional-override sites that had been relying on silent shadowing — the composed builder's Turso and frontend `SutBlockTreeWrite` swaps (`builder.rs`) and two frontend-slice `structural_pbt` builders — all converted to the explicit `CapMap::replace` (panics if the cap was never registered first). `overlay_windowed_caps`' hand-rolled `no-prior-SutDriver` assert was deleted (redundant with `insert`'s panic); `sql_loro_wide` keeps its selective single-cap insert (precedence composition, not a guard). Full lib parity: identical failure set before/after the flip (17 pre-existing slice-teeth reds unchanged, 0 new); keystone green.
- **Open → resolving:** exact split of `Subsystem` (coarse, ~9) vs `Sut*` caps (fine, ~23). Recommendation stands: unify on the capability as the single unit for both `prov` and `need`. **The §8.5 audit gives the first concrete instance:** `min_sut` today encodes *realization* (the structural block invariants carry `TursoProjection` because `E2ESut::SutBackend` reads Turso) rather than the *capability* their bodies actually call (`SutBackend`). **But §8.6 shows this is not a free re-label:** there is only one `SutBackend` realization and it *needs* Turso, so the `TursoProjection` in `min_sut` is currently load-bearing — it gates these invariants out of the non-Turso `loro_backend_pbt` slice, where the SQL would fail. The reclassification (realization→capability) must wait until a non-Turso `SutBackend` realization exists; only then does the parity gate hold. The broader `Subsystem`-vs-cap unification follows from there.

---

## Backlog

The sliced, dependency-ordered task list for extending this — with a 🧠 smart /
🤖 cheap split built for distributing the mechanical work to fast/cheap agents —
lives in [`PbtCompositionBacklog.md`](PbtCompositionBacklog.md). Key finding there:
with the current two components the catalog is *complete*, so coverage now grows by
adding **components** (each a 🧠 scope task that unlocks a batch of 🤖 invariant-adds),
not ref caps.

**Open-contribution item (🧠, not built — §5.5):** make a new subsystem contributable
as a self-contained module (separate crate / git submodule) with zero central edits —
(a) flip transition dispatch to the open `inventory`/`typetag` registry (§8.9, in
flight), and (b) replace `ReferenceState`'s hardcoded subsystem-*private* fields with
an open per-module extension registry (the coupled cross-cutting core stays as-is).
Boundary: additive/private-state subsystems only; a shared-semantics change still
touches the core.

The step-by-step **how** — copy-paste recipes for adding an invariant, a component,
or hosting a cap, with the Needs-from-bounds rule, the test-triad patterns, and the
anti-patterns that fail review — now lives in the **`pbt-composition` skill**
(`.claude/skills/pbt-composition/`), the single source of truth for the migration
*process*. It auto-fires when you edit this code; this document remains the referenced
*architecture* (the §-numbered decisions the skill cites).

## 10. File map

- `crates/holon-pbt-core/src/composition.rs` — the spine (PoC + production seam). Hosts `CapMap` (incl. `expect` = cloned `Arc`, `expect_ref` = borrowed `&C` for borrow-returning cap methods), the read-cap union-adapter (macro-generated), the γ runner (`run_selected` + `Needs::selected_against`), and `BridgedInvariant<I>` (real body → `CapInvariant`).
- `crates/holon-integration-tests/src/pbt/composed/` — **the shared γ catalog** (slice-agnostic; every composed slice runs it). `catalog.rs` = `composed_invariant_catalog()` (one `wire()` line per invariant); `invariants/<name>.rs` = **one file per invariant**: its `Needs`+bridge `wire()` (intrinsic to the body's cap bounds, *not* to any slice) and its `#[cfg(test)]` positive/negative-containment/catch triad driven by `fixtures.rs`'s hand-crafted doubles (no real backend — runs without Turso/Loro/MemoryBackend); `invariants.rs` carries the add-an-invariant recipe. **Adding an invariant = one new `invariants/` file + one `catalog.rs` line**, and it lights up in *every* slice whose components satisfy its `Needs` — the unit a fast/cheap agent owns without touching shared code or any slice.
- `crates/holon-integration-tests/src/pbt/memory_slice/` — Step-2B composed slice = **components only** (the per-slice cost is genuinely different code, not catalog duplication). `components.rs` (`MemoryBackendComponent` over `MemoryBackend` + `InMemEditorComponent` headless editor), `builders.rs` (`memory_wide`/`memory_wide_with_editor`), `integration_tests.rs` (selection-count tests over a real `MemoryBackend`; the block-tree mutation-sequence and editor differential op-sequence proptests; and the **editor commit round-trip** — `take_commit()` → `MemoryBackend::update_block` → `blocks-match` re-reads the store, grounding the editor's text math in real storage). Runs the shared catalog via `run_selected(&composed_invariant_catalog(), &these_components, &ref)`. 8 invariants select; 18 tests green sub-second. A second slice (e.g. Loro) ≈ a new `components.rs` + builder + selection tests — zero catalog repetition.
- `crates/holon-frontend/src/editor_caret.rs` — **pure caret/text arithmetic, the single source of truth** for the byte-offset/UTF-8-boundary math (move-left/right, clamp-to-boundary, byte↔codepoint conversions, insert/delete). Production `headless_editor_mirror::handle_keystroke` calls it (and it dedups the formerly-triplicated inline `chars().count()` conversions); the PBT `InMemEditorComponent` is a thin `String` wrapper over the same primitives — so the editor slice exercises the *real* math, not a parallel copy. The differential oracle (`EditorModel`, `Vec<char>`/codepoint) stays an independent implementation that cross-checks it.
- `crates/holon-integration-tests/src/pbt/loro_slice/` — the **second slice**: a real `holon-loro` `LoroBackend` CRDT (`components.rs` = `LoroBackendComponent` providing `SutBackend` + `SutLoroLog`; `builders.rs` = `loro_wide`; `integration_tests.rs` = selection tests proving the catalog runs over Loro + a Loro mutation-sequence proptest). Same shared catalog, zero duplication — the §6 payoff demonstrated.
- `crates/holon-integration-tests/src/pbt/composed/invariants/loro_no_errors.rs`, `…/loro_children_match_ref.rs` — the two Loro-specific catalog invariants (`SutLoroLog`, and `SutLoroLog + RefBlockTree`); fixture-driven triads via `FixtureLoroLog` (a `SutLoroLog` double that can report an error or mis-order children).
- `crates/holon-integration-tests/src/pbt/invariants/bodies/block_content_matches_ref_backend.rs` — `inv-block-content-matches-ref/block_raw` (`SutBackend + RefBlockTree`); the first body reading the reference via the borrow-returning `RefBlockTree::block_content`.
- `crates/holon-integration-tests/src/pbt/sql_slice/` — the **third slice**: a real Turso `BackendEngine` (production storage + IVM matview layer, **no** reactive engine/frontend/navigation), the storage-layer slice of the future `E2ESut` replacement. `components.rs` = `SqlProjectionComponent` providing `SutBackend` (over `block_raw`, via the shared `parse_block_row`) + `SutSqlProjection` (base-table family real; nav/focus/watch honest-empty); `builders.rs` = `sql_wide` + `new_sql_engine` (the Turso `BackendEngine` with a block-CRUD `SqlOperationProvider`, via `create_test_engine_with_setup`; hoisted here so the combined `sql_loro_slice` reuses it, no copy-paste); `integration_tests.rs` = selection tests proving the catalog runs over Turso + a Turso mutation-sequence proptest. `composed/invariants/block_content_sql.rs` = the SQL-projection content invariant (`SutSqlProjection + RefBlockTree`, id `inv-block-content-matches-ref`), triad via `FixtureSqlProjection`.
- `crates/holon-integration-tests/src/pbt/frontend_slice/` — the **fourth slice**: a real **windowless** `FrontendSession` + `ReactiveEngine` (the production render pipeline via `holon_app::new_from_config_with_di`, no GPUI/geometry), the ViewModel/Renderer slice of the future `E2ESut` replacement. `components.rs` = `HeadlessFrontendComponent` providing `SutRenderer` (real `ensure_watching`→`snapshot`→`interpret_pure`→`view_model_to_snapshot` path), `SutViewModel` (real `headless_error_node_count`; gpui-engine-specific methods honest-`None`), and `SutBackend` (over `block_raw`). `builders.rs` = `frontend_wide` (uses `Config::with_arc` for the once-built session). `composed/invariants/viewmodel_no_error_widgets.rs` = the no-error-widgets invariant (`SutViewModel`, no ref), triad via `FixtureViewModel`. Shared helpers `view_model_to_snapshot` + `override_org_fs_bindings` were made `pub(crate)` for reuse.
- `crates/holon-integration-tests/src/pbt/sql_loro_slice/` — the **fifth slice**: the combined SQL+Loro SUT, the only non-redundant consumer of `SutLoroTaskState`. No new component — `builders.rs` `sql_loro_wide` composes the existing `SqlProjectionComponent` (canonical block store; registers `SutBackend` + `SutSqlProjection`) and `LoroBackendComponent` (registered for **only** `SutLoroTaskState`, avoiding the silent one-provider-per-cap `SutBackend` shadow). `integration_tests.rs` is minimal (positive + catch over the real two-store SUT), reusing the hoisted `sql_slice::builders::new_sql_engine`. The coherence invariant + triad live in `composed/invariants/task_state_storage_coherence.rs` (`SutSqlProjection + SutLoroTaskState`, no ref), triad via `FixtureSqlProjection.task_state` + the new `FixtureLoroTaskState` double.
- `crates/holon-integration-tests/src/pbt/invariants/bodies/block_parent_matches_ref_backend.rs` — `inv-block-parent-matches-ref/block_raw` (`SutBackend + RefBlockTree::parent_of`); closes the re-parent-divergence gap (`blocks-match` skips `Parent`; orphan/cycle only check existence/termination). Sound only where there's no doc-id remapping (the pure-memory slice).
- `crates/holon-macros/src/capmap.rs` — `#[capmap_adapter]`; emits `#[async_trait]` only for traits with async methods (sync `RefBackend`/`RefBlockTree` adapt without it); sync forwards use `CapMap::expect_ref` so borrow-returning methods don't dangle.
- `crates/holon-pbt-core/src/capabilities.rs` — the `Sut*`/`Ref*` cap traits; the SUT read caps carry `#[holon_macros::capmap_adapter]` (Step 0 ✅, + `SutLoroTaskState` for B6), plus the sync caps `RefBackend`, `RefBlockTree`, `RefEditorMirror`, and `SutEditorMirrorRead` (the two `*EditorMirror*` ones host the editor component; `RefBlockTree`/`RefEditorMirror` exercise the `expect_ref` borrow path via `Option<&str>` returns).
- `crates/holon-macros/src/capmap.rs` — the `#[capmap_adapter]` proc-macro that emits the async trait + `CapName` impl + forwarding `impl Trait for CapMap`.
- `crates/holon-pbt-core/src/caching_proxy.rs` — today's per-tick memoisation (→ `CapMap` wrapper).
- `crates/holon-integration-tests/src/pbt/invariant_runner.rs` — the runner. `run_proxy_registry<S>` is the extracted SUT-generic seam (Step 1 ✅); the two `_assert_capmap_*` fns are the compile-only de-risk proofs.
- `crates/holon-integration-tests/src/pbt/sut_capabilities.rs` — `E2ESut`'s cap impls (the tax to dissolve).
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — the omnipotent Ref core.
- `crates/holon-integration-tests/tests/editor_pure_pbt.rs` — narrow hand-written slice (mini-Ref reference).
- `crates/holon/src/api/memory_backend.rs` — Step 2 SUT backing.

## 11. Concept census & concept debt (2026-07-01 audit)

Answer to "do all the concepts have their justification?" — every noun in the system, with a
verdict. Three verdict classes: **KEEP** (load-bearing in the end state), **DIES** (scaffolding
with a named deletion step — justified *only* by that schedule), **DEBT** (no justification in the
end state; needs a unification/deletion decision).

**KEEP — the γ spine.**
- `Sut*`/`Ref*` capability traits, `CapId`, `CapSet` — the unit of provision, need, and selection.
- `CapMap` + `CapProvider` (a.k.a. "component" — one concept, two names; prefer "component" in
  prose, `CapProvider` is just its Rust spelling).
- `#[capmap_adapter]`, `cap_invariant!`/`Needs`/`run_selected`, `cap_transition!`.
- `ReferenceState` (omnipotent core) + `Resolved<_>` witness + `reference_state_ref_caps`.
- `E2ETransition` + `aggregate_transitions` (closed dispatch behind the reversible §8.9 seam).
- `Wiring` (+ `Actor`/`StorageAdapter`, `RequiredWiring`) — the coarse subsystem axis proptest
  draws and shrinks; also the DI boot input.
- The driver ladder (§8.11): `UserDriver`; `DirectUserDriver`/`ReactiveEngineDriver`/
  `GpuiUserDriver`+`SimUserDriver`; `DriverInputComponent`; `DriverPlacement`.
- Captures / `FixtureStep` / `replay_steps` (ADR 0009 "captures, not seeds").
- The ONE PBT: `WideE2E`/`WideE2EMachine`, `wide_e2e_ref*`, `cap_set_for_wiring`,
  `WIDE_REQUIRED_INVARIANTS`; `ComposedSut` harness internals (`from_parts`, `SettleHook`,
  `IdResolver` + scaffold-seed reconcile — the last is load-bearing but documented only in
  `composed/harness.rs`; it deserves a § here eventually).

**DIES — scheduled, with the step that kills it.**
- `Subsystem`/`min_sut`/`PbtSuiteSpec`/native registry + `run_invariant_registry` — E5 increment 5.
- `E2ESut`/`SutHandle`/`sut_capabilities.rs`/`phased.rs`/stepper machinery — E5 increments 4–5.
- The five composed `*_slice`s + `window_slice` builders — §8.10 North Star (each dies as its
  caps live in the ONE PBT).
- `parity.rs` — retired LAST (the deletion gate).
- Fixture doubles (`Fixture*`) — post the no-tests-of-tests directive (2026-06-25) their triad
  duty is retired; they survive only where a slice still references them and die with it.
- `compose_windowed_sut`/`window_input_wide` per-tick E4 hook — folds into the §8.12 windowed
  `ComposedSut` when the harness repoint completes.
- `BridgedInvariant` — an adapter between the statically-typed bodies and `CapInvariant`;
  candidate to fold into `cap_invariant!` once the native registry (the other consumer of the
  static shape) is gone.
- Thin `ReferenceStateMachine` wrappers (17 impls tree-wide; `WindowedRefMachine` is literally
  duplicated in `random_pbt.rs` and `random_pbt_sim.rs`) — they die with the harness repoint;
  the end state has `WideE2EMachine` + genuinely-independent oracles (editor-pure, loro-sync).

**DEBT — no end-state justification; decide, don't drift.**
- **C-1 `Config`'s missing validity check** (§4.2 reality-check box) — **RESOLVED 2026-07-02: bless
  builder asserts, no `req()`.** The audit found no third hand-defended duplicate-guard site, so a
  declarative `req ⊆ P` pass is unwarranted; the validity mechanism is the fail-loud construction path
  (`insert`/`replace` panics + builder asserts). §2 and §4.2 amended accordingly. (`Config` still adds
  little over a `Vec<Arc<dyn CapProvider>>`, but that is an ergonomics nit, not a soundness gap.)
- **C-2 `CapMap::insert` silent overwrite** (§9) — **RESOLVED 2026-07-02: flipped to fail-loud.**
  Added `CapMap::replace` for the four intentional-override sites; parity gate run (17 pre-existing
  reds unchanged, 0 new; keystone green).
- **C-3 Mixed driver rungs in the windowed SUT** (§8.12): re-back the headless-rung write caps
  (`SutFocusWrite`, `SutEditorMirrorWrite`, structural) with the window driver during the harness
  repoint, or disclose per-class exclusions. Includes finishing the `KeystrokeBlockTreeWriter`
  rebind (join/indent/outdent/move still on the dispatch fallback) — and note `SutBlockTreeWrite`
  as a *standing cap* contradicts §8.11's "the floor is a `DirectUserDriver`, not a standing cap";
  it should dissolve into the driver-selection model when the rebind completes.
- **C-4 Four vocabularies for "what is active":** `Subsystem` (dies, E5), `Wiring`, `ComponentSet`
  (= `Wiring` + `projections`, where the composed path *derives* projections via `set_for_wiring`
  — i.e. mostly a function of `Wiring`), `CapSet` (ground truth read off the built `CapMap`).
  Plus the **dual transition gate** (`required_wiring().satisfied_by(wiring) &&
  caps_available(declared_caps)`) and the ref's three-valued `cap_set: Option<CapSet>`
  ("necessary-not-sufficient", `None` = ungated legacy). §9's unify-on-the-capability
  recommendation stands; after E5 kills `Subsystem`, the target is: `Wiring` = the *drawn/shrunk
  input axis*, `CapSet` = the *derived selection truth*, and `ComponentSet` reduced to (or
  replaced by) the `Wiring → components` builder function. Do not add a fifth vocabulary.
- **C-5 Two conventions for capability absence:** (a) don't register the cap → invariant
  deselects (the model §5.1 implies), vs (b) register it "honest-empty" (`sql_slice` nav/focus/
  watch family, `frontend_slice` honest-`None` methods). (b) risks *vacuous green* when a
  selected invariant compares empty-vs-empty — the exact anti-pattern the skill bans. Rule to
  adopt: honest-empty is legitimate only for *individual methods* of a cap whose other methods
  carry the slice's real data AND where no invariant's verdict rests solely on the empty family;
  otherwise split the cap and don't register the absent half. Needs a one-pass audit of existing
  honest-empty methods against the invariants that read them.
  - **AUDIT DONE (2026-07-01):** `docs/Testing/C5_HonestEmptyAudit_2026-07-01.md` (four tiers).
  - **FIXES LANDED (2026-07-02, pbt-target-arch):**
    - *Tier 3 — `SutFocusProjection` cap-split.* `current_focus_rows` / `focus_roots_rows` /
      `nav_history_open_rows` are now their own cap, split off `SutSqlProjection` and registered
      ONLY where navigation is driven (frontend / `full_headless` / navigation slice). A
      storage-only slice (`sql_slice` / `sql_loro_slice`) does not register it, so
      `inv-navigation-focus` / `inv-focus-roots` DESELECT there honestly instead of comparing an
      honest-empty focus family against an unnavigated ref (the exact vacuous-green this rule
      bans). This is the canonical "split the cap, don't register the absent half" pattern.
    - *Tier 1 — `inv-loro-no-errors` real teeth.* `compose_sut` full mode backs `SutLoroLog`
      with the live `LoroSyncControllerHandle` error counter
      (`LoroBackendComponent::new_shared_with_sync_handle`); pure-Loro stays honest-`false`
      (a standalone CRDT genuinely has no controller). The empty family became a real
      data-carrying method in the config that has the capability.
  - **LANDED (2026-07-02, pbt-target-arch — verified via the windowed loop):** Tier 1's three
    emission methods and Tier 2's `frontend_root_vm`/`frontend_root_is_error` are split off
    `SutViewModel` into TWO windowed-only caps:
    - `SutFrontendEmissions` = `live_vs_fresh_tree_diff` / `drain_vm_emission_toggles` /
      `provider_stability_report` (the streaming-emission observer surface; no memoization).
    - `SutFrontendEngine` = `frontend_root_vm` / `frontend_root_is_error` (the root-VM
      resolution surface, ALSO read by `inv-frontend-bounds-rendered`; `frontend_root_vm` carries
      the `CachingProxy` memoization). Two cohesive caps, not one grab-bag: the emission methods
      and the root-VM methods are distinct concerns with distinct consumers/caching.

    Real impls land on `GpuiFrontendEngineComponent` over its live `ReactiveEngine` (faithful
    ports of the deleted `E2ESut` bodies — emission collector spawned once over `engine.watch`,
    persistent `HeadlessLiveTree` cell reused across transitions, twice-interpret provider probe;
    NO engine-side changes needed). Registered on the windowed composition via
    `overlay_windowed_caps` (the deferred base's headless `HeadlessFrontendComponent` does NOT
    provide them — it dropped its honest-`None`/`[]` stubs, so the invariants DESELECT on the
    headless keystone). The `required_invariants` cap_set filter auto-dropped
    `inv-frontend-engine`/`inv-frontend-root-not-error` from the headless floor with NO
    `WIDE_REQUIRED_INVARIANTS` edit — the keystone stays green because the filter no longer
    requires them; the windowed floor keeps them. Verified: all five invariants SELECT + RUN and
    stay GREEN on the windowed loop (`gpui_composed_windowed_loop` + `gpui_compose_sut_windowed`,
    grep `[windowed ran]`), 28 ticks each, zero divergence. This retires the "two conventions"
    debt: the honest convention (don't register the absent half) is now the ONLY one on this
    family too.
- **C-6 Write caps are structurally indistinguishable from read caps** — hence the 14×
  "selection-neutral (no invariant `Needs` it)" comment boilerplate in one `register()` fn.
  Cheap fix candidates: a `WriteCap` marker trait or a `register_write_caps` section convention;
  low priority, documentation-level debt.
