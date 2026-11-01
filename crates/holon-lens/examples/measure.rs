//! Compile and per-value timings of the bundled formats. Run with
//! `cargo run --release -p holon-lens --example measure`.

use std::hint::black_box;
use std::time::Instant;

use holon_lens::Format;
use holon_lens::quantity::Amount;
use holon_lens::quantity::Quantity;
use holon_lens::quantity::Rational;

const TSL: &str = include_str!("../formats/tsl_count.yaml");
const COOK: &str = include_str!("../formats/cooklang_quantity.yaml");

fn time<T>(label: &str, n: u32, mut f: impl FnMut() -> T) {
    let t = Instant::now();
    for _ in 0..n {
        black_box(f());
    }
    println!("{label:<52} {:>10.2?} per call", t.elapsed() / n);
}

fn main() {
    println!("== compile (declaration -> automata + static checks)");
    time("tsl_count (\\p{L})", 50, || Format::from_yaml(TSL).unwrap());
    let ascii = TSL.replace(r"\\p{L}", "a-zA-ZäöüÄÖÜß");
    assert_ne!(ascii, TSL);
    time("tsl_count with ASCII+umlaut letters", 50, || {
        Format::from_yaml(&ascii).unwrap()
    });
    time("cooklang_quantity", 50, || Format::from_yaml(COOK).unwrap());
    let tsl = Format::from_yaml(TSL).unwrap();
    let cook = Format::from_yaml(COOK).unwrap();
    println!(
        "top-level DFA states: tsl {}, cooklang {}",
        tsl.dfa_states(),
        cook.dfa_states()
    );

    println!("\n== parse / print per value");
    for s in [
        "2kg",
        "1,5Kg",
        "0,5Kg + 2 Zehen",
        "0,5Kg + 2 Zehen + 250g + 3 EL",
    ] {
        time(&format!("tsl parse {s:?}"), 20_000, || {
            tsl.parse(s).unwrap()
        });
        let q = tsl.parse(s).unwrap().unwrap();
        time(&format!("tsl put unchanged {s:?}"), 20_000, || {
            tsl.print(Some(&q), Some(s)).unwrap()
        });
        let mut terms = q.terms().to_vec();
        terms[0].amount = Amount::Exact(Rational::int(7));
        let q = Quantity::from_terms(terms).unwrap();
        time(&format!("tsl put first term changed {s:?}"), 20_000, || {
            tsl.print(Some(&q), Some(s)).unwrap()
        });
        time(&format!("tsl create {s:?}"), 20_000, || {
            tsl.print(Some(&q), None).unwrap()
        });
    }
    for s in ["200%g", "1 1/2 % cups", "2-3%cloves", "a pinch"] {
        time(&format!("cooklang parse {s:?}"), 20_000, || {
            cook.parse(s).unwrap()
        });
    }
}
