//! A read call answers either a value or an error; this reads both forms,
//! so a property states which one it expects.

use std::fmt::Debug;

#[derive(Debug)]
pub enum Answer<T> {
    Known(T),
    /// The error form, as text.
    Refused(String),
}

pub trait Form<T> {
    fn answer(self) -> Answer<T>;
}

impl<T> Form<Option<T>> for Option<T> {
    fn answer(self) -> Answer<Option<T>> {
        Answer::Known(self)
    }
}

impl<T, E: Debug> Form<Option<T>> for Result<Option<T>, E> {
    fn answer(self) -> Answer<Option<T>> {
        match self {
            Ok(w) => Answer::Known(w),
            Err(e) => Answer::Refused(format!("{e:?}")),
        }
    }
}

impl<T> Form<Vec<T>> for Vec<T> {
    fn answer(self) -> Answer<Vec<T>> {
        Answer::Known(self)
    }
}

impl<T, E: Debug> Form<Vec<T>> for Result<Vec<T>, E> {
    fn answer(self) -> Answer<Vec<T>> {
        match self {
            Ok(w) => Answer::Known(w),
            Err(e) => Answer::Refused(format!("{e:?}")),
        }
    }
}

pub fn of<T>(form: impl Form<T>) -> Answer<T> {
    form.answer()
}
