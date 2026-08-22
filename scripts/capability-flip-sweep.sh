#!/usr/bin/env bash
# Falsify the org capability profile clause by clause — every clause it
# declares, and every MEMBER of every set-valued clause.
#
# Every clause the certifier CLAIMS to drive must be falsifiable: flip it to a
# value the format does not honour and some counter must move. A flip that
# changes nothing means the clause is decoration — the exact defect the
# certification law exists to prevent.
#
# Three expectations, declared per flip in the table below:
#   move  — a driven clause. Silence is a DEFECT and fails this script.
#   prompt— a driven MEMBER whose removal must be NOTICED by name: the report
#           must raise a TIGHTENING PROMPT, not merely move some counter. Used
#           where "a counter moved" would pass while no finding names the
#           member that went missing.
#   still — a clause deferred to another layer, marked `not_yet_certified`, or
#           a second value that is equally TRUE of the format. Movement is a
#           SURPRISE and fails this script too: it means the classification is
#           wrong, in either direction.
#
# The profile under test is a COPY under target/. This script never writes into
# the source tree.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 90
cd "$ROOT" || exit 90
YAML="crates/holon-org-format/profile.yaml"
# Tree identity: a mis-resolved ROOT must abort BEFORE anything is written.
grep -q '^profile: org' "$YAML" || {
    echo "FATAL: $ROOT/$YAML is not the org capability profile"
    exit 91
}

WORK="target/capability-sweep"
mkdir -p "$WORK" || exit 90
UNDER_TEST="$WORK/profile.yaml"
HONEST="$WORK/profile.honest.yaml"
RUN="$WORK/run.txt"
cp "$YAML" "$HONEST"
cp "$YAML" "$UNDER_TEST"
export HOLON_CAPABILITY_PROFILE="$ROOT/$UNDER_TEST"
# A sweep run that wrote the ledger's input would leave it describing a
# deliberately broken profile.
export HOLON_CAPABILITY_REPORT_DIR="$ROOT/$WORK/reports"

# Build the test binary ONCE, under the semaphore, and run THAT per flip. A
# `cargo` call per flip costs no compilation — the profile is read at runtime —
# but each one queues behind every other lane's build and the sweep starves
# (measured: ~6s per flip, then ~10 MINUTES per flip under contention).
echo "building the certification binary (once)…"
BUILD_JSON="$WORK/build.json"
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id "holon-build-$(hostname -s)" -j4 --fg -- \
    cargo test -p holon-org-format --test profile_certification --no-run --message-format=json \
    >"$BUILD_JSON" 2>"$WORK/build.err" || {
    echo "FATAL: building the certification binary failed — see $WORK/build.err"
    exit 92
}
BIN=$(python3 - "$BUILD_JSON" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line.startswith('{'):
        continue
    m = json.loads(line)
    if m.get('reason') == 'compiler-artifact' and m.get('executable') \
            and m.get('target', {}).get('name') == 'profile_certification':
        print(m['executable'])
        break
PY
)
[ -n "$BIN" ] && [ -x "$BIN" ] || {
    echo "FATAL: could not locate the profile_certification binary in $BUILD_JSON"
    exit 93
}
echo "binary: $BIN"

run() {
    "$BIN" --test-threads=1 --nocapture >"$RUN" 2>&1 || true
    grep "confirmed:" "$RUN" | head -1 | tr -s ' ' | sed 's/^ *//'
}

# The prompts counter alone, so a `prompt` expectation can require a FINDING
# rather than any movement at all.
prompts_of() {
    sed -n 's/.*tightening prompts: \([0-9]*\).*/\1/p' <<<"$1" | head -1
}

BASE=$(run)
BASE_PROMPTS=$(prompts_of "$BASE")
echo "BASELINE: $BASE"
# A blank baseline means the parse below never matched, and every flip would
# then compare empty to empty and read as silent. Fail loudly instead.
[ -n "$BASE" ] || {
    echo "FATAL: baseline empty — the sweep cannot measure (see $RUN)"
    exit 2
}

total=0
defects=0
surprises=0

flip() {
    local from="$1" to="$2" expect="$3"
    python3 - "$UNDER_TEST" "$from" "$to" <<'PY' || {
import sys
p, f, t = sys.argv[1], sys.argv[2], sys.argv[3]
# The table is a flat text file, so a member ADDITION spells its newline.
t = t.replace('\\n', '\n')
s = open(p).read()
if f not in s:
    sys.exit(9)
open(p, 'w').write(s.replace(f, t, 1))
PY
        # A pattern that no longer matches is a RENAMED clause, not a pass: it
        # would silently drop that clause from the sweep.
        echo "FATAL(absent): $from"
        exit 3
    }
    total=$((total + 1))
    local out
    out=$(run)
    if [ "$expect" == "prompt" ]; then
        local now
        now=$(prompts_of "$out")
        if [ -n "$now" ] && [ "$now" -gt "${BASE_PROMPTS:-0}" ]; then
            echo "  prompted $from -> $to"
        else
            echo "  UNNAMED-DEFECT  $from -> $to (no tightening prompt: $BASE_PROMPTS -> ${now:-?})"
            defects=$((defects + 1))
        fi
        cp "$HONEST" "$UNDER_TEST"
        return
    fi
    if [ "$out" == "$BASE" ]; then
        if [ "$expect" == "move" ]; then
            echo "  SILENT-DEFECT  $from -> $to"
            defects=$((defects + 1))
        else
            echo "  still (as declared)  $from -> $to"
        fi
    else
        if [ "$expect" == "move" ]; then
            echo "  moved    $from -> $to"
        else
            echo "  SURPRISE-MOVED  $from -> $to"
            surprises=$((surprises + 1))
        fi
    fi
    cp "$HONEST" "$UNDER_TEST"
}

while IFS='|' read -r from to expect; do
    [ -n "${from:-}" ] || continue
    case "$from" in \#*) continue ;; esac
    flip "$from" "$to" "$expect"
done <<'FLIPS'
charset: no_whitespace|charset: any|move
charset: no_whitespace|charset: identifier|move
charset: no_whitespace|charset: keyword_namespaced|move
case: sensitive|case: folded_upper|move
case: sensitive|case: folded_lower|move
collision: last_wins|collision: first_wins|move
collision: last_wins|collision: error|move
collision: last_wins|collision: multi_valued|move
schema_required: open|schema_required: declared|move
reserved_prefixes: ["_"]|reserved_prefixes: []|move
reserved_prefixes: ["_"]|reserved_prefixes: ["__"]|move
reserved_keys: [ID, TAGS,|reserved_keys: [TAGS,|move
reserved_keys: [ID,|reserved_keys: [Plain, ID,|move
types: [string]|types: [string, integer]|move
types: [string]|types: [integer]|move
empty_string: dropped|empty_string: representable|move
empty_string: dropped|empty_string: error|move
null: dropped|null: representable|move
null: dropped|null: error|move
kind: delimited|kind: none|move
scope: edge_fields_only|scope: all_properties|move
separators: [",", " ", "\t", "\u00a0"]|separators: [" ", "\t", "\u00a0"]|move
separators: [",", " ", "\t", "\u00a0"]|separators: [",", "\t", "\u00a0"]|prompt
separators: [",", " ", "\t", "\u00a0"]|separators: [",", " ", "\u00a0"]|move
separators: [",", " ", "\t", "\u00a0"]|separators: [",", " ", "\t"]|move
separators: [",", " ", "\t", "\u00a0"]|separators: [",", " ", "\t", "\u00a0", ";"]|move
semantics: set|semantics: list|move
reference_values: by_id|reference_values: none|move
reference_values: by_id|reference_values: by_name|still
representation: marked_text|representation: opaque_text|move
representation: marked_text|representation: structured_tree|move
representation: marked_text|representation: none|move
      - table||move
      - logbook||move
      - quote||move
      - list||move
      - heading||move
      - image||move
      - todo_keyword||move
      - priority||move
      - planning_timestamp||move
      - source_block||move
      - bold||move
      - italic||move
      - verbatim||move
      - link_by_id||move
      - tag||move
      - underline||move
      - code||move
      - link_external||move
      - subscript||move
      - superscript||move
      - link_by_name||move
      - paragraph||move
      - strikethrough|      - strikethrough\n      - escape_sequence|move
sibling_order: file_position|sibling_order: fractional_index|move
sibling_order: file_position|sibling_order: explicit_integer|move
sibling_order: file_position|sibling_order: linked_list|move
sibling_order: file_position|sibling_order: unordered|move
order_key_durable: derived|order_key_durable: authored|move
order_key_durable: derived|order_key_durable: carried|move
order_key_durable: derived|order_key_durable: carried_but_reminted|move
property_order: preserved|property_order: canonical|move
property_order: preserved|property_order: unspecified|move
concurrent_insert: positional_conflict|concurrent_insert: stable|still
shape: forest|shape: flat|move
shape: forest|shape: tree|move
shape: forest|shape: dag|move
max_depth: unbounded|max_depth: !limit 2|move
cycles: rejected|cycles: representable|move
reparent: constrained|reparent: free|still
reparent: constrained|reparent: none|still
      - page_tag_requires_page_ancestor||still
      - page_tag_requires_page_ancestor|      - page_tag_requires_page_ancestor\n      - no_slash_in_page_name|still
id_space: opaque_string|id_space: uuid|move
id_space: opaque_string|id_space: path_derived|move
id_space: opaque_string|id_space: name_derived|move
id_space: opaque_string|id_space: none|move
id_origin: authored|id_origin: minted_on_write|move
id_origin: authored|id_origin: derived_from_position|move
id_constraints: [valid_uri_path]|id_constraints: []|move
id_constraints: [valid_uri_path]|id_constraints: [valid_uri_path, no_slash_in_page_name]|move
rename_stability: [file_rename, title_rename, move]|rename_stability: [title_rename, move]|still
rename_stability: [file_rename, title_rename, move]|rename_stability: [file_rename, move]|still
rename_stability: [file_rename, title_rename, move]|rename_stability: [file_rename, title_rename]|still
carriers: [drawer_id, file_keyword_id, path_derived]|carriers: [drawer_id, file_keyword_id]|move
carriers: [drawer_id, file_keyword_id, path_derived]|carriers: [drawer_id, path_derived]|move
carriers: [drawer_id, file_keyword_id, path_derived]|carriers: [file_keyword_id, path_derived]|move
carriers: [drawer_id, file_keyword_id, path_derived]|carriers: [drawer_id, file_keyword_id, path_derived, name_chain]|move
carrier_disagreement: error|carrier_disagreement: precedence_wins|move
computed_live: full|computed_live: none|still
kind: string_only|kind: none|still
expression_closure: computation_plus_script|expression_closure: none|still
write_leg: file|write_leg: absent|move
write_leg: file|write_leg: api|move
write_leg: file|write_leg: in_process|move
unit_of_write: file|unit_of_write: field|move
unit_of_write: file|unit_of_write: entity|move
unit_of_write: file|unit_of_write: container|move
merge_granularity: file|merge_granularity: character|still
conflict_surface: log|conflict_surface: none|still
hosted_kinds: [hierarchical]|hosted_kinds: [hierarchical, free_standing]|move
hosted_kinds: [hierarchical]|hosted_kinds: [free_standing]|move
attachments: inline_reference|attachments: none|move
attachments: inline_reference|attachments: managed_store|move
binary_inline: none|binary_inline: data_uri|move
binary_inline: none|binary_inline: native|move
extensions: [png, |extensions: [|move
extensions: [png, |extensions: [pdf, png, |move
[png, jpg, jpeg|[png, jpeg|move
jpg, jpeg, gif|jpg, gif|move
jpeg, gif, webp|jpeg, webp|move
gif, webp, svg|gif, svg|move
webp, svg, bmp|webp, bmp|move
svg, bmp, ico|svg, ico|move
bmp, ico, tiff|bmp, tiff|move
ico, tiff, tif]|ico, tif]|move
, tif]|]|move
FLIPS

cp "$HONEST" "$UNDER_TEST"
echo "SWEEP: $total flips, $defects silent-defect(s), $surprises surprise(s)"

# Belt and braces: leave the ledger's input describing the HONEST profile, from
# the in-tree yaml, whatever the last flip did.
echo "restoring the ledger's input with a clean certification…"
unset HOLON_CAPABILITY_PROFILE HOLON_CAPABILITY_REPORT_DIR
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id "holon-build-$(hostname -s)" -j4 --fg -- \
    ./scripts/capability-cert.sh >"$WORK/clean-cert.txt" 2>&1 || {
    echo "FATAL: the closing clean certification failed — see $WORK/clean-cert.txt"
    exit 94
}
grep -E "^profile:|confirmed:" "$WORK/clean-cert.txt" | head -3

if [ "$defects" -gt 0 ] || [ "$surprises" -gt 0 ]; then
    echo "FAIL: the profile and the certifier disagree about what is driven"
    exit 1
fi
echo "OK: every driven clause is falsifiable, every excused clause is inert"
