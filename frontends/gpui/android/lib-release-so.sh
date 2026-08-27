#!/usr/bin/env bash
# Sourced by build-release-apk.sh and build-release-aab.sh — the two scripts
# that package the release .so — so both ship an identically prepared library.

# Largest libholon_gpui.so we will package. A stripped build is ~92MB; the store
# rejects the ~760MB unstripped one on size.
MAX_PACKAGED_SO_BYTES=157286400

so_bytes() { wc -c < "$1" | tr -d ' '; }

# Copy the release .so into the packaging tree and strip the COPY, so the cargo
# artifact under target/ stays debuggable. cargo-ndk strips only when copying via
# -o, which these scripts do not use, so the .so still carries its debuginfo.
stage_release_so() {
    local src="$1" dest_dir="$2"
    : "${NDK:?stage_release_so requires NDK}"
    : "${NDK_HOST:?stage_release_so requires NDK_HOST}"

    local strip="$NDK/toolchains/llvm/prebuilt/$NDK_HOST/bin/llvm-strip"
    if [ ! -x "$strip" ]; then
        echo "ERROR: llvm-strip missing or not executable: $strip" >&2
        exit 1
    fi

    local name dest size
    name="$(basename "$src")"
    dest="$dest_dir/$name"
    cp "$src" "$dest"
    echo "$name: $(so_bytes "$src") bytes before strip"
    "$strip" "$dest"
    size="$(so_bytes "$dest")"
    echo "$name: $size bytes after strip (cap $MAX_PACKAGED_SO_BYTES)"

    if [ "$size" -gt "$MAX_PACKAGED_SO_BYTES" ]; then
        echo "ERROR: packaged $name is $size bytes, over the $MAX_PACKAGED_SO_BYTES cap." >&2
        echo "       Expected ~92MB after stripping. Verify llvm-strip ran against $dest." >&2
        exit 1
    fi
}
