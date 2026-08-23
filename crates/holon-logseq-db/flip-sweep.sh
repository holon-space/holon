#!/usr/bin/env bash
# Flip every guard in this crate, one at a time, and require a test to notice.
#
# A guard nothing reds is a guard that can be deleted by accident, and the
# built-in rule in particular is read by two layers — so this sweep is what
# says the two cannot drift apart in silence.
#
#   crates/holon-logseq-db/flip-sweep.sh              # every flip
#   crates/holon-logseq-db/flip-sweep.sh 0 8          # flips [0, 8)
#
# Each flip is applied to a copy-aside original and written back afterwards;
# the script stops if a restored file's sha256 differs from what it read.
#
# Table format: `@@ <name> <src-key>`, then `--old`, then the text to find
# (which must occur EXACTLY once), then `--new`, then the text to put there.
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1
WORK=$(mktemp -d)
# A flip is live on disk while cargo runs, so the restore must survive an
# interrupt — a killed sweep that leaves a guard flipped would be worse than
# no sweep.
restore() {
    if [ -n "${LIVE_FILE:-}" ] && [ -f "$WORK/aside" ]; then
        cp "$WORK/aside" "$LIVE_FILE"
        echo "restored $LIVE_FILE on exit"
    fi
    rm -rf "$WORK"
}
trap restore EXIT INT TERM

# bash 3.2 (what macOS ships) has no associative arrays.
src_file() {
    case "$1" in
    W) echo crates/holon-logseq-db/src/kvs_writer.rs ;;
    B) echo crates/holon-logseq-db/src/built_in.rs ;;
    D) echo crates/holon-logseq-db/src/datoms.rs ;;
    P) echo crates/holon-logseq-db/src/project.rs ;;
    *) echo "unknown source key $1" >&2 && exit 1 ;;
    esac
}

cat > "$WORK/flips.txt" <<'TABLE'
@@ many-assert-adds W
--old
                    if cardinality == Cardinality::One {
--new
                    if true {
@@ redundant-assert-is-a-no-op W
--old
                    if already_held {
                        continue;
                    }
--new
                    if false {
                        continue;
                    }
@@ undeclared-attribute-refused W
--old
                    let cardinality = declared_cardinality(&graph.root.schema, &entry.attribute)?;
--new
                    let cardinality = declared_cardinality(&graph.root.schema, &entry.attribute)
                        .unwrap_or(Cardinality::One);
@@ db-meta-vocabulary-admitted W
--old
        if crate::datoms::is_self_describing(attribute) {
--new
        if false {
@@ one-supersedes-by-position W
--old
                        datoms
                            .retain(|held| !(held.e == entry.entity && held.a == entry.attribute));
--new
                        let _ = &datoms;
@@ ident-block-arm B
--old
    namespace == "block" || namespace.starts_with("logseq")
--new
    namespace.starts_with("logseq")
@@ ident-logseq-arm B
--old
    namespace == "block" || namespace.starts_with("logseq")
--new
    namespace == "block"
@@ leg1-flag B
--old
        (BUILT_IN_FLAG, MarkerValue::True) => true,
--new
        (BUILT_IN_FLAG, MarkerValue::True) => false,
@@ leg2-file-path B
--old
        (FILE_PATH, _) => true,
--new
        (FILE_PATH, _) => false,
@@ leg3-internal-ident B
--old
        (DB_IDENT, MarkerValue::Keyword(name)) => is_internal_ident(name),
--new
        (DB_IDENT, MarkerValue::Keyword(_)) => false,
@@ marker-normalizes-the-ident B
--old
    attribute.ident().trim_start_matches(':')
--new
    attribute.ident()
@@ orphans-excluded W
--old
    let mut datoms = crate::tree::Tree::load(graph, crate::tree::Index::Eavt)?.datoms()?;
--new
    let mut datoms = crate::tree::Tree::load(graph, crate::tree::Index::Eavt)?.datoms()?;
    for row in &graph.rows {
        if row.addr <= 1 {
            continue;
        }
        if let TransitNode::Map(pairs) = &row.node {
            for (k, v) in pairs {
                if !matches!(k, TransitNode::Keyword(k) if k == "keys") {
                    continue;
                }
                if let TransitNode::List(tuples) = v {
                    for tuple in tuples {
                        if let TransitNode::List(slots) = tuple {
                            if let (
                                Some(TransitNode::Int(e)),
                                Some(TransitNode::Keyword(a)),
                                Some(value),
                            ) = (slots.first(), slots.get(1), slots.get(2))
                            {
                                datoms.push(crate::tree::TreeDatom {
                                    e: *e,
                                    a: a.clone(),
                                    v: value.clone(),
                                    tx: 0,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
@@ cardinality-guard W
--old
    assert_pinned_schema_version(&graph.rows)?;
    if declared_cardinality(&graph.root.schema, TITLE)? == Cardinality::Many {
        return Err(RowError::NotCardinalityOne {
            attribute: TITLE.to_string(),
        });
    }

    let diff = before.diff_against(after);
--new
    assert_pinned_schema_version(&graph.rows)?;

    let diff = before.diff_against(after);
@@ schema-version-guard W
--old
    if found == PINNED_SCHEMA_VERSION {
--new
    if true {
@@ no-title-guard W
--old
            None => Err(RowError::NoTitle { entity }),
--new
            None => Ok(String::new()),
@@ atomic-copy-and-swap W
--old
        let flushed = edit_title(&mut staged, entity, &old_title, &new_title)?.1;
--new
        let flushed = match edit_title(&mut staged, entity, &old_title, &new_title) {
            Ok(edit) => edit.1,
            Err(error) => {
                *graph = staged;
                return Err(error);
            }
        };
@@ created-refusal W
--old
    if let Some(uuid) = diff.created.first() {
--new
    if let Some(uuid) = None::<&String> {
@@ removed-refusal W
--old
    if let Some(uuid) = diff.removed.first() {
--new
    if let Some(uuid) = None::<&String> {
@@ reparent-arm W
--old
    if *parent_id != after.parent_id {
--new
    if false {
@@ reorder-arm W
--old
    if *position != after.position {
--new
    if false {
@@ tags-arm W
--old
    if *tags != after.tags {
--new
    if false {
@@ requires-arm W
--old
    if *requires != after.requires {
--new
    if false {
@@ contributes-arm W
--old
    if *contributes_to != after.contributes_to {
--new
    if false {
@@ advice-arm W
--old
    if *advice_suppressed != after.advice_suppressed {
--new
    if false {
@@ properties-arm W
--old
    if *properties != after.properties {
--new
    if false {
@@ stale-base-guard W
--old
        if stored != observed.content {
--new
        if false {
@@ stale-names-the-right-block W
--old
            return Err(RowError::PushBaseStale {
                uuid: uuid.clone(),
--new
            return Err(RowError::PushBaseStale {
                uuid: "a-uuid-this-push-never-saw".to_string(),
@@ tail-replay-at-all W
--old
    for transaction in graph.tail()?.transactions() {
--new
    for transaction in graph.tail()?.transactions().into_iter().take(0) {
@@ tail-retract-replay W
--old
                DatomOp::Retract => datoms.retain(|held| {
                    !(held.e == entry.entity && held.a == entry.attribute && held.v == entry.value)
                }),
--new
                DatomOp::Retract => {}
@@ stale-guard-reads-the-tail W
--old
            datoms: datoms_now(graph)?,
--new
            datoms: crate::tree::Tree::load(graph, crate::tree::Index::Eavt)?.datoms()?,
@@ overflow-detection W
--old
    let overflowed = tail.datom_count() > PINNED_BRANCHING_FACTOR as usize;
--new
    let overflowed = tail.datom_count() >= PINNED_BRANCHING_FACTOR as usize;
@@ flush-point-exact W
--old
    if overflowed {
        flush_tail(graph)?;
    }
--new
    if false {
        flush_tail(graph)?;
    }
@@ built-in-guard W
--old
        if view.is_built_in(entity) {
--new
        if false {
@@ unknown-block-guard W
--old
        let entity = view
            .entity_by_uuid(uuid)
            .ok_or_else(|| RowError::PushUnknownBlock { uuid: uuid.clone() })?;
--new
        let entity = view.entity_by_uuid(uuid).unwrap_or(-1);
@@ classifier-calls-the-shared-rule D
--old
        if crate::built_in::marks_built_in(datom.into()) {
--new
        if false {
@@ classifier-page-leg D
--old
        .filter(|(e, page)| built_in.contains(page) && !built_in.contains(e))
--new
        .filter(|(e, _)| !built_in.contains(e) && false)
@@ classifier-builtin-outranks-block D
--old
                if built_in.contains(&e) {
                    EntityKind::BuiltIn
                } else {
                    EntityKind::Block
                }
--new
                EntityKind::Block
@@ children-refusal D
--old
            if !children.is_empty() {
--new
            if false {
@@ epoch-zero-timestamp P
--old
            created_at: entity.int_value(&LogseqAttr::CreatedAt).unwrap_or(0),
            updated_at: entity.int_value(&LogseqAttr::UpdatedAt).unwrap_or(0),
--new
            created_at: entity.int_value(&LogseqAttr::CreatedAt).unwrap_or(1_755_000_000_000),
            updated_at: entity.int_value(&LogseqAttr::UpdatedAt).unwrap_or(1_755_000_000_000),
@@ class-index-sees-built-ins P
--old
        .filter_map(|(e, entity)| {
--new
        .filter_map(|(e, entity)| {
            if entity
                .one(&LogseqAttr::Raw(":logseq.property/built-in?".to_string()))
                .is_some()
            {
                return None;
            }
TABLE

python3 - "$WORK" <<'PY'
import json, sys, os
work = sys.argv[1]
flips, current, section = [], None, None
for line in open(os.path.join(work, "flips.txt")).read().split("\n"):
    if line.startswith("@@ "):
        _, name, key = line.split()
        current = {"name": name, "key": key, "old": [], "new": []}
        flips.append(current)
        section = None
    elif line == "--old":
        section = "old"
    elif line == "--new":
        section = "new"
    elif current and section:
        current[section].append(line)
for flip in flips:
    flip["old"] = "\n".join(flip["old"])
    flip["new"] = "\n".join(flip["new"])
json.dump(flips, open(os.path.join(work, "flips.json"), "w"))
PY

COUNT=$(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))))" "$WORK/flips.json")
FROM=${1:-0}
TO=${2:-$COUNT}

echo "======================================================================"
# The tree, next to the verdict: an agent shell's cwd can reset between calls,
# and a sweep that measured the wrong workspace reads exactly like one that
# measured this one.
echo "flip sweep: $COUNT flips, running [$FROM, $TO) in $PWD"

# Every verdict below is a DIFFERENCE from this run, so a suite that is
# already red makes the whole sweep meaningless — and a sweep killed partway
# leaves exactly that state behind.
BASELINE=$(cargo nextest run -p holon-logseq-db --no-fail-fast 2>&1)
if grep -qE '^ +FAIL \[|error: could not compile' <<<"$BASELINE"; then
    echo "BASELINE IS NOT GREEN — no flip verdict would mean anything:"
    grep -E '^ +FAIL \[|error: could not compile' <<<"$BASELINE" | sort -u | head
    exit 1
fi
echo "baseline green: $(grep -E 'Summary' <<<"$BASELINE" | tail -1)"
SILENT=()
for ((i = FROM; i < TO; i++)); do
    read -r NAME KEY < <(python3 -c "
import json, sys
flip = json.load(open(sys.argv[1]))[int(sys.argv[2])]
print(flip['name'], flip['key'])" "$WORK/flips.json" "$i")
    FILE=$(src_file "$KEY")
    BEFORE=$(shasum -a 256 "$FILE" | cut -d' ' -f1)
    cp "$FILE" "$WORK/aside"
    LIVE_FILE=$FILE

    if ! python3 -c "
import json, sys
flip = json.load(open(sys.argv[1]))[int(sys.argv[2])]
path = sys.argv[3]
text = open(path).read()
if text.count(flip['old']) != 1:
    sys.exit(1)
open(path, 'w').write(text.replace(flip['old'], flip['new']))" "$WORK/flips.json" "$i" "$FILE"; then
        printf '%-34s -> NO SUCH SITE (the guard moved or was deleted)\n' "$NAME"
        SILENT+=("$NAME")
        cp "$WORK/aside" "$FILE"
        continue
    fi

    OUT=$(cargo nextest run -p holon-logseq-db --no-fail-fast 2>&1)
    [ -n "${FLIP_SWEEP_DUMP:-}" ] && printf '%s' "$OUT" > "$FLIP_SWEEP_DUMP"
    cp "$WORK/aside" "$FILE"
    LIVE_FILE=
    AFTER=$(shasum -a 256 "$FILE" | cut -d' ' -f1)
    if [ "$BEFORE" != "$AFTER" ]; then
        echo "RESTORE FAILED for $FILE — $BEFORE != $AFTER"
        exit 1
    fi

    if grep -qE 'error\[E|error: could not compile' <<<"$OUT"; then
        printf '%-34s -> compile-error (not silent)\n' "$NAME"
        continue
    fi
    NAMES=$(grep -E '^ +FAIL \[' <<<"$OUT" | sed -E 's/.*\] +(\( *[0-9]+\/[0-9]+\) +)?//' | awk '{print $1"::"$2}' | grep -v '^(' | sort -u)
    REDS=$(grep -c . <<<"$NAMES")
    [ -z "$NAMES" ] && REDS=0
    NAMES=$(tr '\n' ' ' <<<"$NAMES")
    if [ "$REDS" -eq 0 ]; then
        printf '%-34s -> SILENT\n' "$NAME"
        SILENT+=("$NAME")
    else
        printf '%-34s -> RED as required (%s test(s)) %s\n' "$NAME" "$REDS" "$NAMES"
    fi
done

echo "======================================================================"
if [ ${#SILENT[@]} -eq 0 ]; then
    echo "SILENT FLIPS: none"
else
    echo "SILENT FLIPS: ${SILENT[*]}"
    exit 1
fi
