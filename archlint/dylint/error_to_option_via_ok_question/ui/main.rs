// Should fire — `parse().ok()?` in an Option-returning fn.
fn parse_first_int(s: &str) -> Option<i32> {
    // ALLOW(ok): intentional anti-pattern under test by this lint
    let n = s.parse::<i32>().ok()?;
    Some(n + 1)
}

// Should NOT fire — same shape but the fn returns Result, so `?` propagates.
fn parse_first_int_result(s: &str) -> Result<i32, std::num::ParseIntError> {
    let n = s.parse::<i32>()?;
    Ok(n + 1)
}

// Should NOT fire — `?` on an actual Option (no Result→Option conversion).
fn first_char(s: &str) -> Option<char> {
    let c = s.chars().next()?;
    Some(c)
}

fn main() {
    let _ = parse_first_int("42");
    let _ = parse_first_int_result("42");
    let _ = first_char("hello");
}
