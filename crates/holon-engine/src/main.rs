use clap::{Parser, Subcommand};
use holon_engine::engine::Engine;
use holon_engine::yaml::{History, YamlMarking, YamlNet};
use holon_engine::{display, objective};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "holon-engine", about = "Petri net engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[arg(long, default_value = ".", global = true)]
    dir: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    State,
    Enabled,
    Step { transition: Option<String> },
    Simulate { n: usize },
    Whatif { transition: String },
    History,
    Validate,
    Reset,
    Objective,
}

fn load_all(dir: &PathBuf) -> Result<(YamlNet, YamlMarking, History), Box<dyn std::error::Error>> {
    let net = YamlNet::load(&dir.join("net.yaml"))?;
    let mut marking = YamlMarking::load(&dir.join("state.yaml"))?;
    let history = History::load(&dir.join("history.yaml"))?;
    history.replay(&mut marking);
    Ok((net, marking, history))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let engine = Engine::new();

    match cli.command {
        Command::State => {
            let (_, marking, _) = load_all(&cli.dir)?;
            display::print_marking(&marking);
        }

        Command::Enabled => {
            let (net, marking, _) = load_all(&cli.dir)?;
            let enabled = engine.enabled(&net, &marking);
            let ranked = engine.rank(&net, &marking, &enabled);
            display::print_ranked(&ranked);
        }

        Command::Step { transition } => {
            let (net, mut marking, mut history) = load_all(&cli.dir)?;
            let enabled = engine.enabled(&net, &marking);

            let binding = if let Some(ref tid) = transition {
                enabled
                    .into_iter()
                    .find(|b| b.transition_id == *tid)
                    .ok_or_else(|| format!("transition '{tid}' is not enabled"))?
            } else {
                let ranked = engine.rank(&net, &marking, &enabled);
                assert!(!ranked.is_empty(), "no transitions enabled");
                ranked.into_iter().next().unwrap().binding
            };

            let step = history.next_step();
            let event = engine.fire(&net, &mut marking, &binding, step)?;
            println!("Fired: {} (step {})", event.transition, event.step);
            for change in &event.changes {
                println!(
                    "  {}.{}: {} → {}",
                    change.token, change.attr, change.from, change.to
                );
            }
            history.append(event);
            history.save(&cli.dir.join("history.yaml"))?;
        }

        Command::Simulate { n } => {
            let (net, mut marking, mut history) = load_all(&cli.dir)?;
            for _ in 0..n {
                let enabled = engine.enabled(&net, &marking);
                if enabled.is_empty() {
                    println!("No more transitions enabled.");
                    break;
                }
                let ranked = engine.rank(&net, &marking, &enabled);
                if ranked.is_empty() {
                    break;
                }
                let binding = ranked.into_iter().next().unwrap().binding;
                let step = history.next_step();
                let event = engine.fire(&net, &mut marking, &binding, step)?;
                println!("Step {}: {}", event.step, event.transition);
                history.append(event);
            }
            history.save(&cli.dir.join("history.yaml"))?;
            println!();
            display::print_marking(&marking);
        }

        Command::Whatif { transition } => {
            let (net, marking, _) = load_all(&cli.dir)?;
            let enabled = engine.enabled(&net, &marking);
            let binding = enabled
                .into_iter()
                .find(|b| b.transition_id == transition)
                .ok_or_else(|| format!("transition '{transition}' is not enabled"))?;

            let evaluator = holon_engine::guard::RhaiEvaluator::new();
            let obj_before = objective::evaluate(&evaluator, &net, &marking)
                .map(|r| r.value)
                .unwrap_or(0.0);

            let mut sim = marking.clone();
            let event = engine.fire(&net, &mut sim, &binding, 0)?;
            let obj_after = objective::evaluate(&evaluator, &net, &sim)
                .map(|r| r.value)
                .unwrap_or(0.0);

            display::print_whatif(&event, obj_before, obj_after);
        }

        Command::History => {
            let history = History::load(&cli.dir.join("history.yaml"))?;
            display::print_history(&history);
        }

        Command::Validate => {
            let net = YamlNet::load(&cli.dir.join("net.yaml"))?;
            let errors = net.validate();
            display::print_validation(&errors);
        }

        Command::Reset => {
            let empty = History { events: vec![] };
            empty.save(&cli.dir.join("history.yaml"))?;
            println!("History cleared.");
        }

        Command::Objective => {
            let (net, marking, _) = load_all(&cli.dir)?;
            let evaluator = holon_engine::guard::RhaiEvaluator::new();
            let result = objective::evaluate(&evaluator, &net, &marking)?;
            display::print_objective(result.value, &result.constraint_violations);
        }
    }

    Ok(())
}
