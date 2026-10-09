//! The part of jq's standard library a mapping compiles against.
//!
//! A filter is offered only when its name is listed in [`ALLOWED`]; a filter a
//! jaq release adds stays unavailable until someone lists it here.

use jaq_core::Native;
use jaq_core::load::parse::Def;
use jaq_core::native::Filter;

use crate::mapping::Data;

/// Filters the jq libraries provide that a mapping may not call, with the
/// reason its refusal states.
pub(crate) const WITHHELD: &[(&str, &str)] = &[
    ("halt", PROCESS),
    ("halt_error", PROCESS),
    (
        "env",
        "it reads the process environment, which holds connection secrets",
    ),
    ("now", CLOCK),
    ("localtime", ZONE),
    ("strflocaltime", ZONE),
    ("debug", LOG),
    ("debug_empty", LOG),
    ("stderr", LOG),
    ("stderr_empty", LOG),
];

/// Why `import` and `include` are refused, whether they name a module or a
/// data file.
pub(crate) const IMPORT: &str = "a mapping is one self-contained filter and loads no module or \
                                 data file";

const PROCESS: &str = "it ends the Holon process";
const CLOCK: &str = "it reads the clock, so one response maps to different rows on every run";
const ZONE: &str = "it reads this machine's time zone, so one response maps to different rows \
                    on different machines";
const LOG: &str = "it writes to the Holon log";

/// Every pure filter of jaq-core, jaq-std and jaq-json, natives and
/// definitions alike. A definition is listed together with every native it
/// calls, so this set compiles on its own.
#[rustfmt::skip]
const ALLOWED: &[&str] = &[
    // jaq-core
    "error_empty", "path", "path_value", "range", "keys_unsorted", "key_values", "first",
    "last", "limit", "skip", "empty", "error", "true", "false", "not", "select", "tostring",
    "repeat", "recurse", "while", "until", "paths", "getpath", "setpath", "delpaths", "map",
    "map_values", "walk", "del", "nth", "join", "combinations", "to_entries", "from_entries",
    "with_entries", "isempty", "all", "any",
    // jaq-std: values, strings, arrays
    "null", "floor", "round", "ceil", "utf8bytelength", "explode", "implode", "ascii_downcase",
    "ascii_upcase", "reverse", "sort", "sort_by", "group_by", "min_by_or_empty",
    "max_by_or_empty", "startswith", "endswith", "ltrimstr", "rtrimstr", "trim", "ltrim",
    "rtrim", "isboolean", "isnumber", "isstring", "isarray", "isobject", "type", "values",
    "nulls", "booleans", "numbers", "finites", "normals", "strings", "arrays", "objects",
    "iterables", "scalars", "add", "min_by", "max_by", "min", "max", "unique_by", "unique",
    "pick", "keys", "flatten",
    // jaq-std: regular expressions
    "matches", "split_matches", "split_", "capture_of_match", "test", "scan", "match",
    "capture", "split", "splits", "sub", "gsub",
    // jaq-std: formats
    "escape_sh", "escape_html", "unescape_html", "encode_uri", "decode_uri", "encode_base64",
    "decode_base64", "@sh", "@text", "@html", "@htmld", "@uri", "@urid", "@base64",
    "@base64d",
    // jaq-std: time, UTC only
    "fromdateiso8601", "todateiso8601", "strftime", "gmtime", "strptime", "mktime", "todate",
    "fromdate",
    // jaq-std: math
    "nan", "infinite", "isnan", "isinfinite", "isfinite", "isnormal", "abs", "logb",
    "significand", "pow10", "drem", "nexttoward", "scalb", "gamma", "acos", "acosh", "asin",
    "asinh", "atan", "atanh", "cbrt", "cos", "cosh", "erf", "erfc", "exp", "exp10", "exp2",
    "expm1", "fabs", "frexp", "ilogb", "j0", "j1", "lgamma", "log", "log10", "log1p", "log2",
    "modf", "nearbyint", "rint", "sin", "sinh", "sqrt", "tan", "tanh", "tgamma", "trunc", "y0",
    "y1", "atan2", "copysign", "fdim", "fmax", "fmin", "fmod", "hypot", "jn", "ldexp",
    "nextafter", "pow", "remainder", "scalbln", "yn", "fma",
    // jaq-json
    "fromjson", "tojson", "tobytes", "length", "contains", "has", "indices", "bsearch",
    "totype", "tonumber", "toboolean", "transpose", "in", "inside", "index", "rindex", "@json",
];

pub(crate) fn withheld_reason(name: &str) -> Option<&'static str> {
    WITHHELD
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

pub(crate) fn defs() -> impl Iterator<Item = Def<&'static str>> {
    all_defs().filter(|d| ALLOWED.contains(&d.name))
}

pub(crate) fn funs() -> impl Iterator<Item = Filter<Native<Data>>> {
    all_funs().filter(|(name, _, _)| ALLOWED.contains(name))
}

fn all_defs() -> impl Iterator<Item = Def<&'static str>> {
    jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs())
}

fn all_funs() -> impl Iterator<Item = Filter<Native<Data>>> {
    jaq_core::funs::<Data>()
        .chain(jaq_std::funs::<Data>())
        .chain(jaq_json::funs::<Data>())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_library_filter_is_either_allowed_or_withheld() {
        let library: BTreeSet<&str> = all_defs()
            .map(|d| d.name)
            .chain(all_funs().map(|(name, _, _)| name))
            .collect();
        let allowed: BTreeSet<&str> = ALLOWED.iter().copied().collect();
        let withheld: BTreeSet<&str> = WITHHELD.iter().map(|(n, _)| *n).collect();

        let both: Vec<_> = allowed.intersection(&withheld).collect();
        assert!(both.is_empty(), "allowed and withheld at once: {both:?}");
        let unclassified: Vec<_> = library
            .iter()
            .filter(|n| !allowed.contains(*n) && !withheld.contains(*n))
            .collect();
        assert!(
            unclassified.is_empty(),
            "the jq libraries provide filters this module neither allows nor withholds: \
             {unclassified:?}"
        );
        let unknown: Vec<_> = allowed
            .union(&withheld)
            .filter(|n| !library.contains(*n))
            .collect();
        assert!(
            unknown.is_empty(),
            "listed names no jq library provides: {unknown:?}"
        );
    }

    #[test]
    fn the_allowed_set_compiles_on_its_own() {
        crate::RowMapper::compile("the allowed set", ".").expect("the allowed set is closed");
    }
}
