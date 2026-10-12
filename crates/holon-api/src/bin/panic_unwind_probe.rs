//! A process built with the profile under test whose task panics under
//! `spawn_disclosing_panic`. Test binaries always unwind, whatever the profile
//! says, so only a separate binary shows what a shipped build does with a
//! panic. Run by `tests/release_build_unwinds.rs`.

use std::sync::Arc;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT;
use holon_api::lifecycle::SessionShutdown;

fn main() {
    let message = std::env::args()
        .nth(1)
        .expect("usage: panic-unwind-probe <panic message>");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the tokio runtime");
    let conditions = runtime.block_on(async move {
        let shutdown = SessionShutdown::new();
        let bus = Arc::new(ConditionBus::new());
        let disclose_to = bus.clone();
        shutdown.spawn_disclosing_panic(
            "panic-unwind-probe",
            async move { panic!("{message}") },
            move |message| {
                disclose_to.emit(Condition {
                    subject: "panic-unwind-probe".to_string(),
                    reason: ConditionKind::TaskPanicked {
                        message: message.to_string(),
                        thread: "panic-unwind-probe".to_string(),
                    },
                })
            },
        );
        shutdown
            .shutdown(DEFAULT_SHUTDOWN_TIMEOUT)
            .await
            .expect("the panicked task is joined");
        bus.current()
    });
    for condition in conditions {
        if let ConditionKind::TaskPanicked { message, .. } = condition.reason {
            println!("disclosed: {message}");
        }
    }
    println!("alive after the panic");
}
