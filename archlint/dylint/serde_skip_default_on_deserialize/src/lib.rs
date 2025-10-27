//! `serde_skip_default_on_deserialize` — flags struct fields that combine
//! `#[serde(skip)]` / `#[serde(skip_deserializing)]` (with or without an
//! explicit `default`) on a type that derives `Deserialize`. Such fields
//! are silently filled with `Default::default()` whenever the type is
//! deserialized, regardless of what the source says.
//!
//! Maps to the May 2026 "Block has two deserializers" regression: the
//! derived `TryFromEntity::from_entity` set `tags = []` on every SQL row →
//! Block conversion because the `tags` field had `#[serde(skip, default)]`.
//! The hand-rolled `Block::try_from(HashMap<String, Value>)` was the
//! correct path; the derived path silently dropped edge data.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_hir;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::{Item, ItemKind, MetaItemInner};
use rustc_lint::{LateContext, LateLintPass};
use rustc_span::Symbol;

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Flags fields of a struct deriving `serde::Deserialize` that carry
    /// `#[serde(skip)]` or `#[serde(skip_deserializing)]` — these fields
    /// are silently filled with `Default::default()` at deserialize time.
    ///
    /// ### Why is this bad?
    ///
    /// A caller doing `serde_json::from_value::<Foo>(...)` gets back a
    /// `Foo` whose skipped fields look populated but are actually defaults.
    /// The May 2026 holon regression had `Block::tags` with
    /// `#[serde(skip, default)]`; the derived `TryFromEntity::from_entity`
    /// always returned `tags = []`, dropping edge data on every SQL → Block
    /// conversion. The hand-rolled `Block::try_from(HashMap<…>)` path was
    /// the correct one, but the silent default made the bug invisible.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// #[derive(serde::Deserialize)]
    /// struct Block {
    ///     id: String,
    ///     #[serde(skip, default)]   // <-- silently `vec![]` on deserialize
    ///     tags: Vec<String>,
    /// }
    /// ```
    ///
    /// Use instead: hydrate the field explicitly in a hand-rolled
    /// constructor (`impl TryFrom<HashMap<…>> for Block`), or remove the
    /// derived `Deserialize` so callers can't accidentally land on the
    /// silently-defaulting path.
    ///
    /// To suppress: `#[allow(serde_skip_default_on_deserialize)]` on the
    /// field — typical when the type really is non-deserialized state
    /// (e.g. a runtime cache that's intentionally empty after JSON load).
    pub SERDE_SKIP_DEFAULT_ON_DESERIALIZE,
    Warn,
    "field with #[serde(skip*)] is silently defaulted on deserialize"
}

fn last_segment_is(meta: &MetaItemInner, name: Symbol) -> bool {
    meta.meta_item()
        .and_then(|m| m.path.segments.last())
        .is_some_and(|seg| seg.ident.name == name)
}

fn struct_derives_deserialize(cx: &LateContext<'_>, item: &Item<'_>) -> bool {
    let deserialize = Symbol::intern("Deserialize");
    let derive = Symbol::intern("derive");
    for attr in cx.tcx.hir_attrs(item.hir_id()) {
        if !attr.has_name(derive) {
            continue;
        }
        let Some(items) = attr.meta_item_list() else {
            continue;
        };
        if items.iter().any(|it| last_segment_is(it, deserialize)) {
            return true;
        }
    }
    false
}

#[derive(Default)]
struct SerdeFlags {
    skip: bool,
    skip_deserializing: bool,
}

fn collect_serde_flags(cx: &LateContext<'_>, hir_id: rustc_hir::HirId) -> SerdeFlags {
    let mut flags = SerdeFlags::default();
    let serde = Symbol::intern("serde");
    let skip = Symbol::intern("skip");
    let skip_de = Symbol::intern("skip_deserializing");
    for attr in cx.tcx.hir_attrs(hir_id) {
        if !attr.has_name(serde) {
            continue;
        }
        let Some(items) = attr.meta_item_list() else {
            continue;
        };
        for it in items {
            if last_segment_is(&it, skip) {
                flags.skip = true;
            } else if last_segment_is(&it, skip_de) {
                flags.skip_deserializing = true;
            }
        }
    }
    flags
}

impl<'tcx> LateLintPass<'tcx> for SerdeSkipDefaultOnDeserialize {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        let ItemKind::Struct(_, _, variant) = &item.kind else {
            return;
        };
        if !struct_derives_deserialize(cx, item) {
            return;
        }
        for field in variant.fields() {
            let flags = collect_serde_flags(cx, field.hir_id);
            if !(flags.skip || flags.skip_deserializing) {
                continue;
            }
            span_lint_and_help(
                cx,
                SERDE_SKIP_DEFAULT_ON_DESERIALIZE,
                field.span,
                "field is silently filled with `Default::default()` at deserialize time",
                None,
                "the type derives `Deserialize` and this field has \
                 `#[serde(skip)]` / `#[serde(skip_deserializing)]`, so \
                 callers receive defaulted data without warning. Either \
                 remove the derived `Deserialize` (force a hand-rolled \
                 constructor that hydrates the field explicitly), or, if \
                 the field really is runtime-only state, document the \
                 intent and add `#[allow(serde_skip_default_on_deserialize)]`. \
                 See devlog/MEMORY: Block has two deserializers, May 2026.",
            );
        }
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
