# `result_to_none`

A type-aware Rust lint (dylint) that flags manual `Result -> Option`
conversions which silently drop the error.

### What it does

Triggers on `match` expressions of the shape

```rust
match expr {
    Ok(x) => Some(x),
    Err(_) => None,
}
```

(or with the arms swapped) where `expr` has type `Result<_, _>`.

### Why is this bad?

The error is silently discarded. Per the Holon project's CLAUDE.md:

> **DO NOT** returning `null` or `None` in case of an error.
> **DO** throw an exception / return an `Err` / `Failure` / ... instead.

Converting a `Result<T, E>` to `Option<T>` at a function boundary loses
typed information about *why* something failed — exactly the bug class the
project's "Fail Loud, Never Fake" philosophy targets.

### Why dylint and not ast-grep?

The `archlint/rules/ok.yml` ast-grep rule catches every `.ok()` call and
relies on a curated allow-list (writeln!, env::var, OnceLock::set, ...) to
filter false positives. `result_to_none` is sharper: it inspects the
scrutinee's type (`Result<_, _>` vs anything else) and only fires on the
exact "drop the error" shape. That shape is unambiguous, so no allow-list
is needed.

### Example

```rust
// Bad
fn parse_int(s: &str) -> Option<i32> {
    match s.parse::<i32>() {
        Ok(n) => Some(n),
        Err(_) => None,
    }
}

// Good
fn parse_int(s: &str) -> Result<i32, std::num::ParseIntError> {
    s.parse::<i32>()
}
```

### Suppressing

Add `#[allow(result_to_none)]` to the function or expression. Suppress only
when the error genuinely is "no information" (e.g. a probe that should
return `Option<bool>` because there's no error type richer than absence).

### Adding more dylint lints

Run from `archlint/dylint/`:

```sh
cargo dylint new --isolate <lint_name>
```

Each lint is a self-contained crate (see `Cargo.toml`'s `[workspace]` empty
table — required by dylint). Update `archlint/archlint.py::cmd_dylint` (or
the wrapper) to load the new lib by name.
