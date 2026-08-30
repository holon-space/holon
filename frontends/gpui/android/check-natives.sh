#!/usr/bin/env bash
# Assert every `native` method of the compiled classes is exported by the .so
# being packaged next to them.
#
# JNI resolves natives lazily, so a class declaring a method the library does
# not export builds, installs and runs — then throws UnsatisfiedLinkError on the
# IME binder thread the first time a user reaches that method. This turns the
# Java half and the gpui-mobile half drifting apart into a build failure.
#
# Usage: check-natives.sh <classes-dir> <library.so> <llvm-nm>
set -euo pipefail

CLASSES="${1:?classes directory}"
SO="${2:?library}"
NM="${3:?llvm-nm}"

command -v javap >/dev/null || { echo "ERROR: javap not on PATH (a JDK is required)" >&2; exit 1; }
[ -x "$NM" ] || { echo "ERROR: nm not found: $NM" >&2; exit 1; }
[ -e "$SO" ] || { echo "ERROR: library not found: $SO" >&2; exit 1; }

EXPORTS="$(mktemp)"
trap 'rm -f "$EXPORTS"' EXIT
"$NM" --defined-only --extern-only "$SO" > "$EXPORTS"

declared=0
missing=0
while IFS= read -r class_file; do
    class="${class_file#"$CLASSES"/}"
    class="${class%.class}"
    fqcn="${class//\//.}"
    while IFS= read -r method; do
        declared=$((declared + 1))
        # JNI's short name escapes `_` in a class or method name as `_1`; no
        # symbol here needs it, so refuse rather than derive the wrong name.
        case "$fqcn$method" in
            *_*) echo "ERROR: $fqcn.$method needs JNI name mangling this check does not implement" >&2; exit 1 ;;
        esac
        symbol="Java_${fqcn//./_}_$method"
        if ! grep -q " $symbol\$" "$EXPORTS"; then
            echo "ERROR: $fqcn.$method() is declared native but $(basename "$SO") exports no $symbol" >&2
            missing=$((missing + 1))
        fi
    done < <(javap -p -cp "$CLASSES" "$fqcn" | grep ' native ' | sed 's/(.*//' | awk '{print $NF}')
done < <(find "$CLASSES" -name '*.class')

[ "$declared" -gt 0 ] || { echo "ERROR: no native methods found in $CLASSES — the check inspected nothing" >&2; exit 1; }
[ "$missing" -eq 0 ] || exit 1
echo "natives: $declared declared, all exported by $(basename "$SO")"
