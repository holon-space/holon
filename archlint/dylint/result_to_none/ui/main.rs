// Type-driven examples for the `result_to_none` lint.

#[derive(Debug)]
struct ParseError;

fn parse_int(s: &str) -> Result<i32, ParseError> {
    s.parse::<i32>().map_err(|_| ParseError)
}

// Should fire — Ok(x) => Some(x), Err(_) => None on a Result.
fn explicit_match_drops_err(s: &str) -> Option<i32> {
    match parse_int(s) {
        Ok(n) => Some(n),
        Err(_) => None,
    }
}

// Should also fire — arms swapped.
fn swapped_arm_order(s: &str) -> Option<i32> {
    match parse_int(s) {
        Err(_) => None,
        Ok(n) => Some(n),
    }
}

// Should NOT fire — Option scrutinee, not Result.
fn option_passthrough(opt: Option<i32>) -> Option<i32> {
    match opt {
        Some(n) => Some(n),
        None => None,
    }
}

// Should NOT fire — error is propagated, not dropped.
fn propagates_error(s: &str) -> Result<i32, ParseError> {
    match parse_int(s) {
        Ok(n) => Ok(n),
        Err(e) => Err(e),
    }
}

fn main() {
    let _ = explicit_match_drops_err("42");
    let _ = swapped_arm_order("7");
    let _ = option_passthrough(Some(1));
    let _ = propagates_error("3");
}
