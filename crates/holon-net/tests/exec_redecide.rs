//! A quiescence decides an auto transition again only for the subjects whose
//! read rows changed since its last decision, and always for those.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Read;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;

const NOW: Duration = Duration::ZERO;

/// `Follow` fires for a waiting subject once its bind sees what it needs;
/// `binds` counts the bind's runs.
fn follow_net(bind: Read) -> NetDef {
    NetDef::default()
        .place("Waiting", Class::Process)
        .place("Flag", Class::Process)
        .place("Done", Class::Process)
        .transition(
            TransitionDef::new(
                "Follow",
                Trigger::Auto,
                bind,
                Body::pure(|s, _| Ok(vec![Output::new("Done", s, Content::new())])),
            )
            .reads("Waiting")
            .inhibits("Done"),
        )
}

fn on() -> Content {
    Content::from([("on".to_string(), Value::Boolean(true))])
}

fn write(ex: &mut Executor, place: &str, subject: &str, content: Content) {
    drop(ex.write(place, subject, content, NOW).unwrap());
}

fn done(ex: &Executor) -> Vec<String> {
    ex.tokens()
        .subjects("Done")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn a_write_to_a_row_another_subject_read_decides_that_subject_again() {
    let flag_of_a: Read = Arc::new(|r, _| r.field("Flag", "a", "on").map(|_| Content::new()));
    let mut ex = Executor::new(follow_net(flag_of_a)).unwrap();
    write(&mut ex, "Waiting", "b", Content::new());
    assert_eq!(done(&ex), Vec::<String>::new());
    write(&mut ex, "Flag", "a", on());
    assert_eq!(done(&ex), ["b"], "b read Flag of a, so its write enables b");
}

#[test]
fn a_write_to_its_own_read_row_decides_the_subject_again() {
    let own_flag: Read = Arc::new(|r, s| {
        (r.field("Waiting", s, "on") == Some(Value::Boolean(true))).then(Content::new)
    });
    let mut ex = Executor::new(follow_net(own_flag)).unwrap();
    write(&mut ex, "Waiting", "b", Content::new());
    assert_eq!(done(&ex), Vec::<String>::new());
    write(&mut ex, "Waiting", "b", on());
    assert_eq!(done(&ex), ["b"]);
}

#[test]
fn a_write_decides_only_the_subjects_that_read_the_written_row() {
    let binds = Arc::new(AtomicUsize::new(0));
    let counted = binds.clone();
    let own_flag: Read = Arc::new(move |r, s| {
        counted.fetch_add(1, Ordering::Relaxed);
        r.field("Flag", s, "on").map(|_| Content::new())
    });
    let mut ex = Executor::new(follow_net(own_flag)).unwrap();
    for n in 0..100 {
        write(&mut ex, "Waiting", &format!("s{n}"), Content::new());
    }
    let before = binds.load(Ordering::Relaxed);
    write(&mut ex, "Flag", "s7", on());
    assert_eq!(done(&ex), ["s7"]);
    assert_eq!(
        binds.load(Ordering::Relaxed) - before,
        1,
        "only s7 read the written row"
    );
}
