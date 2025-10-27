use crate::engine::RankedTransition;
use crate::yaml::history::History;
use crate::{Marking, TokenState};

pub fn print_marking<M: Marking>(marking: &M) {
    println!("Clock: {}", marking.clock());
    println!();
    for token in marking.tokens() {
        println!("  {} [{}]", token.id(), token.token_type());
        for (k, v) in token.attrs() {
            println!("    {k}: {v}");
        }
    }
}

pub fn print_ranked(ranked: &[RankedTransition]) {
    if ranked.is_empty() {
        println!("No transitions enabled.");
        return;
    }
    println!(
        "{:<4} {:<30} {:>10} {:>12}",
        "#", "Transition", "Δobj", "Δobj/min"
    );
    println!("{}", "-".repeat(60));
    for (i, rt) in ranked.iter().enumerate() {
        println!(
            "{:<4} {:<30} {:>10.3} {:>12.4}",
            i + 1,
            rt.binding.transition_id,
            rt.delta_obj,
            rt.delta_per_minute,
        );
        if !rt.binding.token_bindings.is_empty() {
            let bindings: Vec<String> = rt
                .binding
                .token_bindings
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!("     bindings: {}", bindings.join(", "));
        }
    }
}

pub fn print_history(history: &History) {
    if history.events.is_empty() {
        println!("No events recorded.");
        return;
    }
    for event in &history.events {
        println!(
            "Step {}: {} at {} ({}min)",
            event.step, event.transition, event.time, event.duration
        );
        for change in &event.changes {
            println!(
                "  {}.{}: {} → {}",
                change.token, change.attr, change.from, change.to
            );
        }
    }
}

pub fn print_validation(errors: &[String]) {
    if errors.is_empty() {
        println!("Net is valid.");
    } else {
        println!("Validation errors:");
        for e in errors {
            println!("  - {e}");
        }
    }
}

pub fn print_objective(value: f64, violations: &[String]) {
    println!("Objective value: {value:.4}");
    if violations.is_empty() {
        println!("All constraints satisfied.");
    } else {
        println!("Constraint violations:");
        for v in violations {
            println!("  - {v}");
        }
    }
}

pub fn print_whatif(event: &crate::yaml::history::Event, obj_before: f64, obj_after: f64) {
    println!("What-if: {}", event.transition);
    println!("  Duration: {}min", event.duration);
    println!(
        "  Objective: {obj_before:.4} → {obj_after:.4} (Δ = {:.4})",
        obj_after - obj_before
    );
    for change in &event.changes {
        println!(
            "  {}.{}: {} → {}",
            change.token, change.attr, change.from, change.to
        );
    }
}
