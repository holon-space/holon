// Should fire — bare `Result<i32, ()>` return type.
fn returns_unit_error() -> Result<i32, ()> {
    Err(())
}

// Should fire — same shape via `let` binding annotation.
fn binding_annotation() {
    let r: Result<i32, ()> = Ok(0);
    let _ = r;
}

// Should NOT fire — proper error type.
#[derive(Debug)]
struct ParseErr;
fn returns_real_error() -> Result<i32, ParseErr> {
    Err(ParseErr)
}

// Should NOT fire — error wrapped in box.
fn returns_boxed() -> Result<i32, Box<dyn std::error::Error>> {
    Ok(1)
}

fn main() {
    let _ = returns_unit_error();
    binding_annotation();
    let _ = returns_real_error();
    let _ = returns_boxed();
}
